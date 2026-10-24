//! 桌面分区窗口管理：透明分区窗口、位置持久化、分区吸附、拖放归集、桌面右键菜单
use crate::db::{BoxWindowCfg, Db};
use crate::engine;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

/// 分区窗口注册表：box_id -> window label
pub struct BoxWindows {
    map: Mutex<HashMap<i64, String>>,
    last_save: Mutex<HashMap<i64, Instant>>,
    dragging: Mutex<HashMap<i64, bool>>,
    /// 最近一次移动/调整大小得到的几何（x,y,w,h），用于窗口销毁/程序退出时
    /// 把"最终位置"落盘——否则节流会丢掉拖动的最后一帧，重启后布局跳回中途位置。
    geom: Mutex<HashMap<i64, (i32, i32, i32, i32)>>,
}

impl Clone for BoxWindows {
    fn clone(&self) -> Self {
        BoxWindows {
            map: Mutex::new(self.map.lock().map(|m| m.clone()).unwrap_or_default()),
            last_save: Mutex::new(
                self.last_save.lock().map(|m| m.clone()).unwrap_or_default(),
            ),
            dragging: Mutex::new(self.dragging.lock().map(|m| m.clone()).unwrap_or_default()),
            geom: Mutex::new(self.geom.lock().map(|m| m.clone()).unwrap_or_default()),
        }
    }
}

impl BoxWindows {
    pub fn new() -> Self {
        BoxWindows {
            map: Mutex::new(HashMap::new()),
            last_save: Mutex::new(HashMap::new()),
            dragging: Mutex::new(HashMap::new()),
            geom: Mutex::new(HashMap::new()),
        }
    }

    /// 尝试标记某分区"正在拖动吸附"，已在拖动中则返回 false（防止吸附线程叠加）
    pub fn try_begin_drag(&self, box_id: i64) -> bool {
        let mut d = match self.dragging.lock() {
            Ok(x) => x,
            Err(_) => return false,
        };
        if d.get(&box_id).copied().unwrap_or(false) {
            return false;
        }
        d.insert(box_id, true);
        true
    }

    pub fn end_drag(&self, box_id: i64) {
        if let Ok(mut d) = self.dragging.lock() {
            d.remove(&box_id);
        }
    }

    pub fn register(&self, box_id: i64, label: &str) {
        if let Ok(mut m) = self.map.lock() {
            m.insert(box_id, label.to_string());
        }
    }

    pub fn unregister(&self, box_id: i64) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(&box_id);
        }
        if let Ok(mut s) = self.last_save.lock() {
            s.remove(&box_id);
        }
        if let Ok(mut d) = self.dragging.lock() {
            d.remove(&box_id);
        }
        if let Ok(mut g) = self.geom.lock() {
            g.remove(&box_id);
        }
    }

    /// 记录某分区最近一次几何（x,y,w,h），不阻塞、不触发落盘。
    pub fn cache_geom(&self, box_id: i64, x: i32, y: i32, w: i32, h: i32) {
        if let Ok(mut g) = self.geom.lock() {
            g.insert(box_id, (x, y, w, h));
        }
    }

    /// 取出并清掉缓存的几何（窗口销毁时调用，落盘最终位置）。
    pub fn take_geom(&self, box_id: i64) -> Option<(i32, i32, i32, i32)> {
        if let Ok(mut g) = self.geom.lock() {
            g.remove(&box_id)
        } else {
            None
        }
    }

    pub fn label_for(&self, box_id: i64) -> Option<String> {
        self.map.lock().ok()?.get(&box_id).cloned()
    }

    pub fn ids(&self) -> Vec<i64> {
        self.map
            .lock()
            .map(|m| m.keys().copied().collect())
            .unwrap_or_default()
    }

    fn should_save(&self, box_id: i64) -> bool {
        if let Ok(mut s) = self.last_save.lock() {
            let now = Instant::now();
            if let Some(last) = s.get(&box_id) {
                if now.duration_since(*last) < Duration::from_millis(500) {
                    return false;
                }
            }
            s.insert(box_id, now);
        }
        true
    }
}

/// 分区顶层窗口的 HWND（裸值，shelldrop 兜底做矩形命中用；HWND 不是 Send，
/// 只在取到后立刻用，不跨线程保存）。
pub fn hwnd_for_box(app: &AppHandle, box_id: i64) -> Option<isize> {
    let label = app.state::<BoxWindows>().label_for(box_id)?;
    let win = app.get_webview_window(&label)?;
    let h = win.hwnd().ok()?;
    Some(h.0 as isize)
}

fn persist_cfg(window: &WebviewWindow, box_id: i64, force: bool) {
    let app = window.app_handle();
    let Some(bw) = app.try_state::<BoxWindows>() else { return };
    let Ok(pos) = window.outer_position() else { return };
    let Ok(size) = window.outer_size() else { return };
    let (x, y, w, h) = (pos.x, pos.y, size.width as i32, size.height as i32);
    // 每次移动/调整都缓存最新几何：窗口销毁或程序退出时（节流可能已丢掉最后一帧）
    // 用缓存把"最终位置"落盘，重启后布局才和退出前完全一致。
    bw.cache_geom(box_id, x, y, w, h);
    if !force && !bw.should_save(box_id) {
        return;
    }
    let visible = window.is_visible().unwrap_or(true);
    // 只改几何字段，且在 DB 的同一把锁内读-改-写（见 Db::update_box_window_cfg）：
    // 前端刚保存的图标大小/视图模式/手排顺序不会被覆盖，并发写也不会互相丢更新。
    //
    // 胶囊模式（collapsed = true）下只跟踪 x/y，**不覆盖 w/h**：此时窗口是悬停
    // 临时展开 / 胶囊尺寸，真正要保住的是 expanded_geom（收起前的几何）。
    // 若照常写 w/h，悬停展开会把"展开尺寸"当成"分区尺寸"写进 cfg，展开语义就乱了。
    //
    // 多屏绑定：把窗口中心点所在屏的硬件身份（PnP 实例路径）和屏内比例位置
    // 一起写进配置。下次启动 / 分辨率变了 / 屏幕排列换了，分区都按
    // "归属屏 + 比例"找回自己的位置，而不是靠绝对坐标硬凑。
    let (cx, cy) = (x + w / 2, y + h / 2);
    let mon = monitor_at_point(cx, cy);
    let rel = mon.as_ref().map(|m| rel_pos(m.work, x, y, w, h));
    let mon_key = mon.map(|m| m.key);
    let db = app.state::<Db>();
    let _ = db.update_box_window_cfg(box_id, |c| {
        c.x = x;
        c.y = y;
        if !c.collapsed {
            c.w = w;
            c.h = h;
        }
        c.visible = visible;
        if let Some(key) = mon_key {
            c.monitor_key = key;
            c.rel = rel;
        }
    });
}

/// 强制窗口不在任务栏显示：设置 WS_EX_TOOLWINDOW 并清除 WS_EX_APPWINDOW
pub fn force_no_taskbar(win: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
        };
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                // GetWindowLongPtrW 在 32 位下返回 i32、64 位下返回 isize，统一转 isize 兼容两架构
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as isize;
                let new_ex = (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
                let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex as _);
            }
        }
    }
    let _ = win;
    // 显示桌面免疫只作用于桌面常驻组件/分区。搜索面板、分区菜单要置顶弹在
    // 最上层，绝不能挂成桌面子窗口——WS_CHILD 下 always_on_top 无效，
    // 面板会被其它程序盖住（之前搜索面板"不在最上层"就是这个原因）。
    if is_desktop_widget(win.label()) {
        let keep = win
            .app_handle()
            .state::<Db>()
            .get_setting("app")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<crate::commands::AppSettings>(&s).ok())
            .map(|x| x.keep_on_show_desktop)
            .unwrap_or(true);
        apply_show_desktop_immune(win, keep);
    }
}

/// 让窗口「免疫」任务栏右侧「显示桌面」（即 Win+D）。
///
/// 原理：把窗口挂为桌面窗口（WorkerW / Progman）的子窗口，显示桌面只最小化
/// 顶层应用窗口，子窗口不受影响，于是便签 / 股票 / 分区等保持常驻桌面。
/// `keep=false` 时恢复为普通顶层窗口（点显示桌面会随其它窗口一起最小化）。
/// 是否启用由设置页「显示桌面时保持组件和分区可见」控制。
/// 已安装「防最小化」子类过程的窗口句柄集合，避免重复安装
#[cfg(target_os = "windows")]
static SUBCLASSED: LazyLock<Mutex<std::collections::HashSet<isize>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

#[cfg(target_os = "windows")]
const SUBCLASS_ID: usize = 0x5149_444F; // "QIDO"

/// 「我方主动隐藏」的窗口句柄 → 标记时刻（毫秒）。一次性许可，见 [`hide_win`]。
///
/// 带时间戳是为了防"许可卡死"：若某次隐藏实际没生效（窗口仍是可见的），许可会一直
/// 挂着，该窗口就永久失去保护（显示桌面时会被系统弄走且不记录任何日志）。守护线程
/// 会定期清掉"标记超过 1.5 秒、窗口却仍然可见"的陈旧许可。
#[cfg(target_os = "windows")]
static USER_HIDDEN: LazyLock<Mutex<std::collections::HashMap<isize, u64>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

/// 进程启动时刻，用于给「主动隐藏许可」打时间戳。
#[cfg(target_os = "windows")]
static EPOCH: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);

#[cfg(target_os = "windows")]
fn now_ms() -> u64 {
    EPOCH.elapsed().as_millis() as u64
}

#[cfg(target_os = "windows")]
fn is_user_hidden(hwnd: windows::Win32::Foundation::HWND) -> bool {
    USER_HIDDEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&(hwnd.0 as isize))
}

#[cfg(target_os = "windows")]
fn mark_user_hidden(hwnd: windows::Win32::Foundation::HWND) {
    USER_HIDDEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(hwnd.0 as isize, now_ms());
}

#[cfg(target_os = "windows")]
fn clear_user_hidden(hwnd: windows::Win32::Foundation::HWND) {
    USER_HIDDEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(hwnd.0 as isize));
}

/// 我方主动隐藏窗口（托盘「最小化」、收起分区、关闭组件等）。
///
/// 「显示桌面免疫」子类会否决**隐藏**与**最小化**，但用户自己要求的隐藏必须放行：
/// 否则托盘菜单里的「最小化」点下去会立刻被补显示的兜底逻辑弹回来，表现为
/// 「菜单里的最小化没反应」，而系统「显示桌面」又能把窗口弄走 —— 完全反了。
///
/// 这里先登记一张一次性许可再 `hide()`，子类看到许可就直接放行；窗口下次
/// 重新显示（`WM_SHOWWINDOW wp=1`）时许可自动失效，免疫能力恢复。
pub fn hide_win(win: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    if let Ok(hwnd) = win.hwnd() {
        mark_user_hidden(hwnd);
    }
    let _ = win.hide();
}

/// 同 [`hide_win`]，用于只能在回调里拿到 `Window`（非 `WebviewWindow`）的场合。
pub fn hide_window(win: &tauri::Window) {
    #[cfg(target_os = "windows")]
    if let Ok(hwnd) = win.hwnd() {
        mark_user_hidden(hwnd);
    }
    let _ = win.hide();
}

/// 子类过程：吞掉最小化 + 否决「显示桌面」隐藏。
///
/// Win10：显示桌面走 `WM_SYSCOMMAND/SC_MINIMIZE` 或 `ShowWindow(SW_MINIMIZE)`，
/// 最终到 `WM_SIZE(wParam==SIZE_MINIMIZED)`，拦掉即可。
/// Win11（探针实测）：Win+D 根本不发最小化消息，而是直接 `ShowWindow(SW_HIDE)`
/// 把窗口藏起来（窗口依次收到 `WM_SHOWWINDOW(wp=0)` + 带 `SWP_HIDEWINDOW` 的
/// `WM_WINDOWPOSCHANGING`）。因此在 `WM_WINDOWPOSCHANGING` 里摘掉该标志从源头
/// 否决隐藏，并用延迟补显示兜底，组件/分区始终留在桌面上。
#[cfg(target_os = "windows")]
unsafe extern "system" fn no_minimize_subclass(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _uid: usize,
    _ref_data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::DefSubclassProc;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, IsIconic, IsWindowVisible, KillTimer, PostMessageW, SetTimer,
        ShowWindow, GWL_EXSTYLE, SET_WINDOW_POS_FLAGS, SWP_HIDEWINDOW, SW_SHOWNOACTIVATE,
        WINDOWPOS, HWND_BOTTOM, WS_EX_NOACTIVATE,
    };

    const WM_SIZE: u32 = 0x0005;
    const WM_SYSCOMMAND: u32 = 0x0112;
    const WM_SHOWWINDOW: u32 = 0x0018;
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    const WM_TIMER: u32 = 0x0113;
    // WM_MOUSEACTIVATE：鼠标点击时系统来问"要不要因此激活窗口"
    const WM_MOUSEACTIVATE: u32 = 0x0021;
    const MA_NOACTIVATE: usize = 3;
    // WM_APP+1：隐藏完成后补一次显示的兜底消息
    const WM_APP_RESHOW: u32 = 0x8001;
    // 补显示之后再核一次的定时器（外壳可能在补显示之后才真正把窗口藏掉）
    const RESHOW_TIMER_ID: usize = 0x5149_4450; // "QIDP"
    const SIZE_MINIMIZED: usize = 1;
    const SC_MINIMIZE: usize = 0xF020;

    // 0) 兜底核验：补显示后 300ms 再确认一次，若又被藏掉/最小化则再救回来。
    //    放在最前面（早于"主动隐藏许可"的放行），保证定时器一定被注销，不会空转。
    if msg == WM_TIMER && wparam.0 == RESHOW_TIMER_ID {
        let _ = KillTimer(Some(hwnd), RESHOW_TIMER_ID);
        if !is_user_hidden(hwnd) && restore_window(hwnd) {
            crate::log_msg(&format!(
                "no-minimize: 兜底核验再次救回窗口 hwnd={:x}",
                hwnd.0 as usize
            ));
        }
        return LRESULT(0);
    }
    // 0.05) 带 WS_EX_NOACTIVATE 的分区窗口：点击不激活、不置顶，鼠标消息照常送达。
    //       这是「点击分区 → 全部分区闪一下 / 双击没反应」的根修（配合
    //       apply_show_desktop_immune 里打的样式标志）。显式返回比依赖系统
    //       对该样式的默认处理更可靠（WebView2 宿主链上的激活路径各版本有差异）。
    if msg == WM_MOUSEACTIVATE {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as isize;
        if ex & (WS_EX_NOACTIVATE.0 as isize) != 0 {
            return LRESULT(MA_NOACTIVATE as isize);
        }
    }
    // 0.1) 窗口重新显示 → 撤销「我方主动隐藏」许可，恢复显示桌面免疫
    if msg == WM_SHOWWINDOW && wparam.0 != 0 {
        clear_user_hidden(hwnd);
    }
    // 0.2) 我方主动隐藏的窗口一律放行（托盘「最小化」/ 收起分区 / 关闭组件）
    if is_user_hidden(hwnd) {
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }

    // 1) 从源头否决隐藏：摘掉 SWP_HIDEWINDOW，SetWindowPos 就不会执行隐藏；
    //    同时拦截「压到最底」——把窗口压到 HWND_BOTTOM 会沉到桌面（Progman）
    //    之下，等于被桌面盖住，因此把目标位置改写为「桌面组正上方」。
    if msg == WM_WINDOWPOSCHANGING && lparam.0 != 0 {
        let pos = &mut *(lparam.0 as *mut WINDOWPOS);
        if pos.flags.contains(SWP_HIDEWINDOW) {
            pos.flags = SET_WINDOW_POS_FLAGS(pos.flags.0 & !SWP_HIDEWINDOW.0);
            crate::log_msg(&format!(
                "no-minimize: 已否决显示桌面隐藏 hwnd={:x}",
                hwnd.0 as usize
            ));
            // 这是「显示桌面」的指纹：外壳马上就会把 Progman 抬到最前把我们整块盖住，
            // 立刻进高频核验窗口，别等轮询周期（否则会看到"闪一下"）
            kick_desktop_keeper_burst();
        }
        if pos.hwndInsertAfter == HWND_BOTTOM {
            if let Some(top) = desktop_group_top() {
                use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GW_HWNDPREV};
                // hWndInsertAfter 是「插到这个窗口下面」，所以取桌面组顶层之上的那个；
                // 取不到（桌面组已在最顶）用 HWND_TOP。
                pos.hwndInsertAfter = GetWindow(top, GW_HWNDPREV)
                    .ok()
                    .filter(|h| !h.is_invalid())
                    .unwrap_or(windows::Win32::Foundation::HWND(std::ptr::null_mut()));
                crate::log_msg(&format!(
                    "no-minimize: 已拦截压底，改插桌面组之上 hwnd={:x}",
                    hwnd.0 as usize
                ));
                kick_desktop_keeper_burst();
            }
        }
    }
    // 2) 兜底：WM_SHOWWINDOW(wp=0) 到达说明有隐藏意图，隐藏完成后补显示
    //    并修复 z 序（防止已被压到桌面之下）
    if msg == WM_SHOWWINDOW && wparam.0 == 0 {
        crate::log_msg(&format!(
            "no-minimize: 检测到隐藏意图(WM_SHOWWINDOW wp=0) hwnd={:x}",
            hwnd.0 as usize
        ));
        let _ = PostMessageW(Some(hwnd), WM_APP_RESHOW, WPARAM(0), LPARAM(0));
    }
    if msg == WM_APP_RESHOW {
        let was_visible = IsWindowVisible(hwnd).as_bool();
        let was_iconic = IsIconic(hwnd).as_bool();
        if !was_visible || was_iconic {
            // SW_SHOWNOACTIVATE 对被最小化的窗口同样会精确还原到原尺寸/位置
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        raise_above_wallpaper(hwnd);
        // 外壳枚举窗口的顺序不固定：可能在我们补显示之后才把窗口真正藏掉，
        // 300ms 后再核一次，确保最终状态是"可见"
        let _ = SetTimer(Some(hwnd), RESHOW_TIMER_ID, 300, None);
        crate::log_msg(&format!(
            "no-minimize: 重显示检查完成 hwnd={:x} 之前可见={} 之前最小化={}",
            hwnd.0 as usize, was_visible, was_iconic
        ));
        return LRESULT(0);
    }
    // Win10 路径：拦最小化命令与最小化尺寸
    if msg == WM_SYSCOMMAND && (wparam.0 & 0xFFF0) == SC_MINIMIZE {
        crate::log_msg(&format!(
            "no-minimize: 已否决最小化命令(SC_MINIMIZE) hwnd={:x}",
            hwnd.0 as usize
        ));
        return LRESULT(0);
    }
    if msg == WM_SIZE && wparam.0 == SIZE_MINIMIZED {
        // 不在最小化消息里同步还原（外壳的最小化操作可能尚未完成，会被随后覆盖），
        // 改为投递一条延迟消息，等最小化落地后再还原，命中率最高
        crate::log_msg(&format!(
            "no-minimize: 检测到被最小化(WM_SIZE)，安排还原 hwnd={:x}",
            hwnd.0 as usize
        ));
        let _ = PostMessageW(Some(hwnd), WM_APP_RESHOW, WPARAM(0), LPARAM(0));
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// 类名 "Progman" / "WorkerW" 的 UTF-16 常量（避免每次枚举都分配字符串）。
#[cfg(target_os = "windows")]
const PROGMAN_U16: [u16; 7] = [80, 114, 111, 103, 109, 97, 110];
#[cfg(target_os = "windows")]
const WORKERW_U16: [u16; 7] = [87, 111, 114, 107, 101, 114, 87];

/// 「桌面组」的顶层窗口：Progman，以及紧贴它之上的一串 WorkerW（壁纸承载层）。
///
/// 桌面组件必须待在**这个组之上**才不会被桌面盖住；组本身对普通应用不可见。
#[cfg(target_os = "windows")]
unsafe fn desktop_group_top() -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetClassNameW, GetWindow, GW_HWNDPREV,
    };
    let progman = FindWindowW(w!("Progman"), None).ok()?;
    if progman.is_invalid() {
        return None;
    }
    let mut top = progman;
    loop {
        let Ok(prev) = GetWindow(top, GW_HWNDPREV) else {
            break;
        };
        if prev.is_invalid() {
            break;
        }
        let mut buf = [0u16; 32];
        let n = GetClassNameW(prev, &mut buf);
        if &buf[..n.max(0) as usize] != WORKERW_U16.as_slice() {
            break;
        }
        top = prev;
    }
    Some(top)
}

/// 把窗口插到「桌面组」正上方：紧贴壁纸、又位于所有普通窗口之下。
#[cfg(target_os = "windows")]
unsafe fn raise_above_wallpaper(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindow, SetWindowPos, GW_HWNDPREV, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER,
        SWP_NOSENDCHANGING, SWP_NOSIZE,
    };
    let Some(top) = desktop_group_top() else {
        return;
    };
    // 注意 SetWindowPos 的 hWndInsertAfter 表示「插到这个窗口**下面**」，
    // 所以要取桌面组顶层之上的那个窗口；取不到（桌面组已在最顶）则用 HWND_TOP。
    let above = GetWindow(top, GW_HWNDPREV)
        .ok()
        .filter(|h| !h.is_invalid() && *h != hwnd);
    let _ = SetWindowPos(
        hwnd,
        above,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSENDCHANGING,
    );
}

/// 窗口若被藏起或被最小化，则无激活地还原到原尺寸/位置，并把 z 序抬回壁纸层上方。
/// 返回是否真的做了还原（供日志判断）。
#[cfg(target_os = "windows")]
unsafe fn restore_window(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindowVisible, ShowWindow, SW_SHOWNOACTIVATE,
    };
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        raise_above_wallpaper(hwnd);
        return true;
    }
    false
}

/// 「桌面层守护」：保证受保护的桌面组件窗口永远位于桌面组之上。
///
/// 为什么必须这么做（实测结论）：Win11 的「显示桌面」既不发最小化消息、也不发隐藏
/// 消息——它是**把桌面窗口 Progman 抬到几乎所有普通窗口之上**。分区/组件窗口平时
/// 只比 Progman 高一层，于是被整块盖住：`IsWindowVisible=true`、`IsIconic=false`、
/// cloak=0，屏幕上却什么都看不到（用户感知就是"面板被最小化了"）。
/// 消息层拦截在这种情况下完全无从下手，只能靠守护线程把窗口抬回桌面组之上。
///
/// 守护同时做**反向核验**：面板的本分是「桌面之上、所有普通窗口之下」。「显示桌面」
/// 之后外壳把 Progman 沉回底部时，面板可能留在被抬高过的位置——之后新开的软件窗口
/// 就全压在面板下面（正向核验看不见这种故障）。核验时若发现「桌面组之上有可见的
/// 普通窗口、却在面板之下」，就把面板整体沉回桌面组之上。
///
/// 而"抬回来"必须**够快**：慢一拍就表现为"按显示桌面时面板闪一下"（早期 120ms 轮询
/// 实测被盖住 36ms 以上）。所以这里用三路唤醒把响应压到 1 帧以内：
///   1. 进程内信号——子类听到「显示桌面」的指纹（外壳正要隐藏窗口 / 把窗口压到底）
///      就立刻催一次核验，零延迟；
///   2. WinEvent 钩子——`EVENT_OBJECT_REORDER`：Progman 被抬高的瞬间就送达；
///   3. 高频兜底——被触发后的 600ms 内以 4ms 周期核验，兜住外壳的操作时序。
#[cfg(target_os = "windows")]
static KEEPER_SCAN: LazyLock<Mutex<Vec<(isize, u8)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// 守护线程的唤醒句柄（`SetEvent` 即催一次核验）；0 表示尚未创建。
#[cfg(target_os = "windows")]
static KEEPER_KICK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// 退出时置位：守护线程看到后立即停止一切窗口操作。
///
/// 退出阶段窗口在逐个销毁，守护线程若还在对它们做跨线程 `SetWindowPos` /
/// `ShowWindow` / `EnumWindows`，这些调用要靠向主线程发消息来完成——主线程
/// 正埋头销毁 WebView 时就形成互相等待（SendMessage 互锁），表现为
/// 「图标都回桌面了、窗口也该关了，程序就是不肯退」。退出前必须叫停。
#[cfg(target_os = "windows")]
static KEEPER_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 通知桌面守护线程停止（退出流程专用，幂等）。
#[cfg(target_os = "windows")]
pub fn stop_desktop_keeper() {
    KEEPER_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// 非 Windows 平台无守护线程，空实现保持调用方简单。
#[cfg(not(target_os = "windows"))]
pub fn stop_desktop_keeper() {}

/// 高频核验窗口的截止时刻（相对 [`EPOCH`] 的毫秒数）。
#[cfg(target_os = "windows")]
static KEEPER_BURST_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 最近一次唤醒守护线程的时刻（诊断用：量"从触发到窗口归位"的延迟）。
#[cfg(target_os = "windows")]
static KEEPER_LAST_KICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 被触发后保持高频核验的时长。
#[cfg(target_os = "windows")]
const BURST_MS: u64 = 600;
/// 高频核验周期：约 1/4 帧，足以让"被盖住"短到看不见。
#[cfg(target_os = "windows")]
const BURST_POLL_MS: u32 = 4;
/// 闲时兜底核验周期。
#[cfg(target_os = "windows")]
const IDLE_POLL_MS: u32 = 120;
/// 诊断心跳周期：只为证明守护线程还活着（正常时 5 分钟一条，不刷日志）。
#[cfg(target_os = "windows")]
const HEARTBEAT_MS: u64 = 300_000;

/// 进入高频核验窗口并立刻唤醒守护线程。
#[cfg(target_os = "windows")]
fn kick_desktop_keeper_burst() {
    use std::sync::atomic::Ordering;
    KEEPER_BURST_UNTIL.fetch_max(now_ms() + BURST_MS, Ordering::Relaxed);
    kick_desktop_keeper();
}

/// 只唤醒守护线程（不延长高频窗口）。
#[cfg(target_os = "windows")]
fn kick_desktop_keeper() {
    use std::sync::atomic::Ordering;
    KEEPER_LAST_KICK.store(now_ms(), Ordering::Relaxed);
    let h = KEEPER_KICK.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            let _ = windows::Win32::System::Threading::SetEvent(
                windows::Win32::Foundation::HANDLE(h as *mut std::ffi::c_void),
            );
        }
    }
}

/// 枚举回调：按 z 序记录桌面组（Progman=1 / WorkerW=2）、受保护窗口（3）
/// 与合格的普通窗口（0）的位置；不合格的普通窗口不记录（见下方判定）。
/// LPARAM 传入 `*const HashSet<isize>`（受保护窗口集合），避免在回调里加锁。
#[cfg(target_os = "windows")]
unsafe extern "system" fn keeper_scan_cb(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
    let key = hwnd.0 as isize;
    let targets = &*(lparam.0 as *const std::collections::HashSet<isize>);
    let kind = if targets.contains(&key) {
        Some(3u8)
    } else {
        let mut buf = [0u16; 32];
        let n = GetClassNameW(hwnd, &mut buf);
        let cls = &buf[..n.max(0) as usize];
        if cls == PROGMAN_U16.as_slice() {
            Some(1)
        } else if cls == WORKERW_U16.as_slice() {
            Some(2)
        } else if is_qualifying_normal_window(hwnd) {
            // 普通窗口也要记：守护要靠它发现「面板浮在普通窗口之上」的反向故障
            Some(0)
        } else {
            None
        }
    };
    if let Some(kind) = kind {
        if let Ok(mut v) = KEEPER_SCAN.lock() {
            v.push((key, kind));
        }
    }
    windows::core::BOOL(1)
}

/// 「面板不得压住的普通窗口」判定：可见、未最小化、非置顶（WS_EX_TOPMOST 的窗口
/// 在另一个 z 波段，永远在非置顶面板之上，不算故障）、未被 cloak（UWP 挂起 /
/// 虚拟桌面切换的窗口实际看不见）、且不属于本进程。
#[cfg(target_os = "windows")]
unsafe fn is_qualifying_normal_window(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongW, GetWindowThreadProcessId, GWL_EXSTYLE, IsIconic, IsWindowVisible,
        WS_EX_TOPMOST,
    };
    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == std::process::id() {
        return false;
    }
    if (GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0 {
        return false;
    }
    let mut cloaked = 0u32;
    let queried = DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut std::ffi::c_void,
        std::mem::size_of::<u32>() as u32,
    )
    .is_ok();
    if queried && cloaked != 0 {
        return false;
    }
    true
}

/// WinEvent 回调：z 序变化 / 最小化状态变化时被调用。
///
/// 只管"叫醒"守护线程——真正的枚举与修复在主循环里做（系统要求钩子回调尽快返回，
/// 而且在回调里动窗口容易和外壳的操作时序打架）。
#[cfg(target_os = "windows")]
unsafe extern "system" fn keeper_win_event(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    _hwnd: windows::Win32::Foundation::HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    kick_desktop_keeper();
}

/// 启动守护线程（进程内只启动一次）。
#[cfg(target_os = "windows")]
fn ensure_desktop_keeper() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| unsafe { desktop_keeper_loop() });
    });
}

/// 守护线程主循环：等"信号或超时"→ 抽干消息队列（WinEvent 回调在这一步被调用）
/// → 核验一次。闲时 120ms 一轮，被触发后 600ms 内 4ms 一轮。
#[cfg(target_os = "windows")]
unsafe fn desktop_keeper_loop() {
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        CancelWaitableTimer, CreateEventW, CreateWaitableTimerExW, SetWaitableTimer,
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
    };
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
        MWMO_INPUTAVAILABLE, PM_REMOVE, QUEUE_STATUS_FLAGS, QS_ALLINPUT, EVENT_OBJECT_REORDER,
        EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, WINEVENT_OUTOFCONTEXT,
        WINEVENT_SKIPOWNPROCESS,
    };

    // 1) 唤醒事件（自动重置）：三路信号任意一路 SetEvent 就立刻核验一次
    let kick = CreateEventW(None, false, false, None).unwrap_or_default();
    if !kick.is_invalid() {
        KEEPER_KICK.store(kick.0 as isize, Ordering::Relaxed);
    }

    // 2) 高精度可等待定时器：不抬高系统全局定时器频率也能拿到 ~2ms 等待精度
    let timer = CreateWaitableTimerExW(
        None,
        None,
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
        TIMER_ALL_ACCESS.0,
    )
    .or_else(|_| CreateWaitableTimerExW(None, None, 0, TIMER_ALL_ACCESS.0))
    .ok();

    // 3) WinEvent 钩子：Progman 被抬高的瞬间送达（跳过本进程，防止自己动窗口自我触发）
    //    句柄必须一直存活，否则钩子会被系统摘掉——所以绑在循环外的变量上。
    let _hooks = [
        SetWinEventHook(
            EVENT_OBJECT_REORDER,
            EVENT_OBJECT_REORDER,
            None,
            Some(keeper_win_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        ),
        SetWinEventHook(
            EVENT_SYSTEM_MINIMIZESTART,
            EVENT_SYSTEM_MINIMIZEEND,
            None,
            Some(keeper_win_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        ),
    ];

    crate::log_msg(&format!(
        "desktop-keeper: 守护线程已启动 timer={} kick={:#x}",
        timer.is_some(),
        kick.0 as usize
    ));

    loop {
        let burst = now_ms() < KEEPER_BURST_UNTIL.load(Ordering::Relaxed);
        let wait = if burst { BURST_POLL_MS } else { IDLE_POLL_MS };
        let mut handles: Vec<HANDLE> = Vec::with_capacity(2);
        if let Some(t) = timer {
            // 负数 = 相对时间，单位 100ns
            let due = -((wait as i64) * 10_000);
            if SetWaitableTimer(t, &due, 0, None, None, false).is_ok() {
                handles.push(t);
            }
        }
        if !kick.is_invalid() {
            handles.push(kick);
        }
        // QS_EVENT(0x2000) 是 WinEvent 的投递通道，不在 QS_ALLINPUT 里，必须显式并上
        let _ = MsgWaitForMultipleObjectsEx(
            if handles.is_empty() { None } else { Some(&handles) },
            wait,
            QUEUE_STATUS_FLAGS(QS_ALLINPUT.0 | 0x2000),
            MWMO_INPUTAVAILABLE,
        );
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        if let Some(t) = timer {
            // 取消掉本轮定时器：否则已到期的信号会让下一轮立即返回、空转
            let _ = CancelWaitableTimer(t);
        }
        // 核验过程一律捕获 panic：守护线程是"面板可见性"的唯一保障，
        // 绝不能因为一次异常就整条线程悄悄消失（那样面板被盖住后再也回不来）。
        if std::panic::catch_unwind(|| unsafe { desktop_keeper_tick() }).is_err() {
            crate::log_msg("desktop-keeper: 核验过程发生 panic，已捕获并继续运行");
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn desktop_keeper_tick() {
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, EnumWindows, GetWindow, IsIconic,
        IsWindowVisible, SetWindowPos, ShowWindow, GW_HWNDPREV, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOOWNERZORDER, SWP_NOSENDCHANGING, SWP_NOSIZE, SW_SHOWNOACTIVATE,
    };

    // 退出流程已叫停：窗口正在销毁，此时任何跨线程窗口操作都有互锁卡死风险
    if KEEPER_STOP.load(Ordering::Relaxed) {
        return;
    }

    let targets: std::collections::HashSet<isize> = SUBCLASSED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if targets.is_empty() {
        return;
    }

    // 清理陈旧许可：标记超过 1.5 秒、窗口却仍然可见 → 那次隐藏其实没生效，
    // 许可必须丢掉，否则该窗口会永久失去保护（显示桌面时被弄走且不留日志）。
    let ts = now_ms();
    USER_HIDDEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|k, t| {
            if ts.saturating_sub(*t) < 1500 {
                return true;
            }
            !IsWindowVisible(HWND(*k as *mut std::ffi::c_void)).as_bool()
        });

    {
        let mut v = KEEPER_SCAN.lock().unwrap_or_else(|e| e.into_inner());
        v.clear();
    }
    let _ = EnumWindows(Some(keeper_scan_cb), LPARAM(&targets as *const _ as isize));

    let scan = KEEPER_SCAN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // 枚举顺序即 z 序（从上到下），桌面组的顶端 = Progman 及其上方连续的 WorkerW 中最高那个
    let Some(p) = scan.iter().position(|(_, k)| *k == 1) else {
        return;
    };
    let mut top_idx = p;
    while top_idx > 0 && scan[top_idx - 1].1 == 2 {
        top_idx -= 1;
    }
    let group_top = HWND(scan[top_idx].0 as *mut std::ffi::c_void);

    static LAST_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static LAST_HEARTBEAT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut fixed_vis = 0usize;
    let mut need_z: Vec<HWND> = Vec::new();
    let mut below_total = 0usize;
    let mut skipped_hidden = 0usize;
    // 最上层的受保护窗口在扫描结果里的下标（scan 按 z 序从上到下）
    let mut first_prot: Option<usize> = None;

    for (i, (key, kind)) in scan.iter().enumerate() {
        if *kind != 3 {
            continue;
        }
        let hwnd = HWND(*key as *mut std::ffi::c_void);
        if is_user_hidden(hwnd) {
            skipped_hidden += 1;
            continue;
        }
        // 展开中的饼型分区是**故意置顶**的（文件要浮在其它窗口之上可点），
        // 守护别把它拽回桌面组，也别把它算进"最上层受保护窗口"；
        // 收回时会自己沉回桌面层。
        if is_topmost(hwnd) {
            continue;
        }
        if first_prot.is_none() {
            first_prot = Some(i);
        }
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            fixed_vis += 1;
        }
        if i > top_idx {
            need_z.push(hwnd);
            below_total += 1;
        }
    }

    // 反向核验：面板的本分是「桌面之上、所有普通窗口之下」。但「显示桌面」之后外壳把
    // Progman 抬到所有普通窗口之上，守护为保可见把面板抬到了 Progman 之上；随后外壳把
    // Progman 沉回底部时，面板却留在被抬高过的位置 —— **之后新开的软件窗口全压在面板
    // 下面**。正向核验（面板在桌面组之下）看不见这种故障，必须反向查：桌面组之上
    // （i < top_idx）有合格的普通窗口、它却在某个面板之下（i > first_prot）→
    // 把面板整体沉回桌面组之上。
    // 「显示桌面」进行中不会误触发：那时普通窗口全在桌面组之下（i > top_idx），不满足条件。
    let mut normal_above = 0usize;
    if let Some(fp) = first_prot {
        for (i, (_, kind)) in scan.iter().enumerate() {
            if *kind == 0 && i > fp && i < top_idx {
                normal_above += 1;
            }
        }
        if normal_above > 0 {
            for (key, kind) in scan.iter() {
                if *kind != 3 {
                    continue;
                }
                let hwnd = HWND(*key as *mut std::ffi::c_void);
                if is_user_hidden(hwnd) || is_topmost(hwnd) || need_z.contains(&hwnd) {
                    continue;
                }
                need_z.push(hwnd);
            }
        }
    }

    // 批量重排：`DeferWindowPos` 把若干个窗口的 z 序变更合成**一次**原子提交。
    // 逐个 `SetWindowPos` 会让每个窗口各自产生一次合成更新（4 个分区就是 4 次闪烁），
    // 合批之后整组"整体闪现"一次，配合亚帧级响应基本看不到。
    //
    // ⚠️ 这里**不能**带 `SWP_NOSENDCHANGING`：`DeferWindowPos` 不支持它，会直接返回
    // NULL / ERROR_INVALID_PARAMETER(87)（实测）。早期版本正因为带了它，合批永远失败，
    // 而失败又被静默吞掉，导致"按显示桌面面板就再也不回来"。
    // 合批只是"优化"、不是正确性前提：万一仍失败，下面会逐个窗口兜底。
    let mut fixed_z = 0usize;
    let mut defer_failed = false;
    if !need_z.is_empty() {
        let above = GetWindow(group_top, GW_HWNDPREV)
            .ok()
            .filter(|h| !h.is_invalid());
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
        // hWndInsertAfter 表示"插到这个窗口下面"，同一个 `above` 反复用即彼此保持相对次序
        if let Ok(mut hdwp) = BeginDeferWindowPos(need_z.len() as i32) {
            for hwnd in need_z.iter().rev() {
                match DeferWindowPos(hdwp, *hwnd, above, 0, 0, 0, 0, flags) {
                    Ok(next) => {
                        hdwp = next;
                        fixed_z += 1;
                    }
                    Err(_) => {
                        defer_failed = true;
                        break;
                    }
                }
            }
            if EndDeferWindowPos(hdwp).is_err() {
                defer_failed = true;
                fixed_z = 0;
            }
        } else {
            defer_failed = true;
        }
        if fixed_z == 0 {
            // 兜底：逐个 SetWindowPos，保证"能救回来"这件事不依赖合批
            for hwnd in need_z.iter() {
                let _ = SetWindowPos(
                    *hwnd,
                    above,
                    0,
                    0,
                    0,
                    0,
                    flags | SWP_NOSENDCHANGING,
                );
                fixed_z += 1;
            }
            crate::log_msg(&format!(
                "desktop-keeper: 批量重排不可用（{}），已降级为逐个置顶 {} 个窗口",
                if defer_failed { "失败" } else { "空批" },
                fixed_z
            ));
        }
    }

    if fixed_z + fixed_vis > 0 {
        // 刚把窗口救回来，接着的几百毫秒继续盯紧（外壳抬桌面往往分几步走）
        KEEPER_BURST_UNTIL.fetch_max(now_ms() + BURST_MS, Ordering::Relaxed);
        let now = now_ms();
        let last = LAST_LOG.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= 2000 {
            LAST_LOG.store(now, Ordering::Relaxed);
            let kick_at = KEEPER_LAST_KICK.load(Ordering::Relaxed);
            crate::log_msg(&format!(
                "desktop-keeper: 把 {} 个窗口抬回桌面组之上、还原 {} 个被隐藏/最小化的窗口{}（触发后 {}ms）",
                fixed_z,
                fixed_vis,
                if normal_above > 0 {
                    format!(
                        "；{} 个可见普通窗口压在面板上，已整体沉回",
                        normal_above
                    )
                } else {
                    String::new()
                },
                if kick_at == 0 {
                    -1
                } else {
                    now.saturating_sub(kick_at) as i64
                }
            ));
        }
    }

    // 诊断心跳：证明守护线程活着，并暴露它"看见"了什么
    let now = now_ms();
    if now.saturating_sub(LAST_HEARTBEAT.load(Ordering::Relaxed)) >= HEARTBEAT_MS {
        LAST_HEARTBEAT.store(now, Ordering::Relaxed);
        crate::log_msg(&format!(
            "desktop-keeper: 心跳 枚举={} 桌面组顶idx={} 目标={} 组下方={} 普通窗压板={} 主动隐藏={} 待修={}",
            scan.len(),
            top_idx,
            targets.len(),
            below_total,
            normal_above,
            skipped_hidden,
            need_z.len()
        ));
    }
}

#[cfg(target_os = "windows")]
fn set_no_minimize(hwnd: windows::Win32::Foundation::HWND, enable: bool) {
    use windows::Win32::UI::Shell::{RemoveWindowSubclass, SetWindowSubclass};
    unsafe {
        let key = hwnd.0 as isize;
        // 锁中毒也要继续：一旦 panic 就永久失效，之后建的窗口全都不会装防最小化
        let mut set = SUBCLASSED.lock().unwrap_or_else(|e| e.into_inner());
        if enable {
            if set.insert(key) {
                let _ = SetWindowSubclass(hwnd, Some(no_minimize_subclass), SUBCLASS_ID, 0);
            }
        } else if set.remove(&key) {
            let _ = RemoveWindowSubclass(hwnd, Some(no_minimize_subclass), SUBCLASS_ID);
            // 卸载子类时一并丢掉"主动隐藏许可"：HWND 数值会被系统复用，
            // 残留的许可会让新窗口的隐藏/最小化不被保护
            clear_user_hidden(hwnd);
        }
    }
    // 只要还有受保护窗口，就确保守护线程在跑（它负责把窗口压在桌面组之上）
    if enable {
        ensure_desktop_keeper();
    }
}

/// 读注册表 `CurrentBuildNumber`（Win10/11 通用版本号来源）。
/// 不用 GetVersionExW：后者受应用清单里嵌的兼容性声明影响，在 Win10/11 上
/// 可能谎报 6.2。
#[cfg(target_os = "windows")]
fn windows_current_build() -> Option<u32> {
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    unsafe {
        let mut buf = [0u16; 16];
        let mut len = (buf.len() * 2) as u32;
        let ok = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        );
        if ok.is_err() {
            return None;
        }
        String::from_utf16_lossy(&buf)
            .trim_end_matches('\0')
            .trim()
            .parse::<u32>()
            .ok()
    }
}

/// Win11 24H2（build 26100）及以上返回 true。
///
/// 24H2 起（微软 2025-06 的桌面合成改造，为支持 HDR 壁纸等新场景）桌面呈现
/// 方式变了：`Progman` 顶层窗口改用 `WS_EX_NOREDIRECTIONBITMAP` 创建（根本
/// 没有 GDI 重定向表面），挂在它下面的普通子窗口不再享受 DWM 的逐像素
/// alpha 合成——WebView2 的透明像素直接对着黑色重定向表面合成，整个面板
/// 变成一整块黑（现象和 Win10 一样，但根因不同）。微软官方给的适配方式：
/// 嵌入桌面的窗口必须带 `WS_EX_LAYERED` 样式并调用
/// `SetLayeredWindowAttributes(bAlpha=0xFF)`，DWM 才会对它做 alpha 合成。
#[cfg(target_os = "windows")]
pub fn is_win11_24h2_or_later() -> bool {
    windows_current_build().map(|b| b >= 26100).unwrap_or(false)
}

/// 读注册表 `EnableTransparency`（设置→个性化→颜色→透明效果）。
/// 仅用于诊断日志：该开关关闭时任务栏/开始菜单失去亚克力效果，部分机器上
/// 也会影响应用的透明窗口合成。
#[cfg(target_os = "windows")]
fn transparency_effects_enabled() -> Option<bool> {
    use windows::core::w;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
    };
    unsafe {
        let mut v: u32 = 0;
        let mut len = 4u32;
        let ok = RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("EnableTransparency"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut len),
        );
        if ok.is_err() {
            return None;
        }
        Some(v != 0)
    }
}

pub fn apply_show_desktop_immune(win: &WebviewWindow, keep: bool) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, GWL_EXSTYLE, WS_CHILD, WS_POPUP,
            WS_MINIMIZEBOX,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, SetParent, SetWindowPos, SWP_FRAMECHANGED, SWP_NOMOVE,
            SWP_NOSIZE, SWP_NOACTIVATE,
        };
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                // 分区窗口（box-*）设为「点击不激活」：点击只把鼠标消息交给 WebView2，
                // 不激活窗口、不把它抬到最顶层。否则每一下点击都会走
                // 「激活→升顶→焦点事件触发 sink_desktop_widget 整体沉回」，
                // 表现为所有分区闪一下、双击打开的第一下被打断（点开没反应）。
                // 需要键盘输入时（改名 / 分区内搜索）由前端显式 setFocus 申请。
                // 其余桌面组件（便签等）有"点进去就打字"的输入框，保持原行为。
                let no_activate = keep && win.label().starts_with("box-");
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as isize;
                let new_ex = if no_activate {
                    ex | (WS_EX_NOACTIVATE.0 as isize)
                } else {
                    ex & !(WS_EX_NOACTIVATE.0 as isize)
                };
                if new_ex != ex {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex as _);
                }
                if keep {
                    // 统一走「顶层窗口 + 防最小化」方案（全系统版本一致）。
                    //
                    // 历史：曾把窗口挂成 Progman 的 WS_CHILD 子窗口来做「显示桌面免疫」，
                    // 但逐像素 alpha 合成对子窗口的支持因机器而异——实测同一份程序，
                    // Win11 26H1 正常、25H2(build 26200) 面板整块黑，且加 WS_EX_LAYERED
                    // 也救不回来；Win10 则完全不给子窗口做透明合成。而透明窗口依赖的
                    // DwmEnableBlurBehindWindow 只对顶层窗口生效，所以顶层窗口的透明
                    // 在所有机器上都可靠。防最小化子类过程已能挡住 Win+D / SC_MINIMIZE /
                    // SW_MINIMIZE，「显示桌面免疫」效果不变，于是彻底弃用挂桌面子窗口。
                    crate::log_msg(&format!(
                        "show-desktop-immune: label={} build={:?} win24h2={} transparency={:?} mode=top-level",
                        win.label(),
                        windows_current_build(),
                        is_win11_24h2_or_later(),
                        transparency_effects_enabled()
                    ));
                    // 兜底：直接吞掉最小化，无论显示桌面用什么方式实现都不会最小化窗口
                    set_no_minimize(hwnd, true);
                    // 从源头掐掉「最小化」这条路径：去掉 WS_MINIMIZEBOX，
                    // 系统「显示桌面」在遍历窗口时会跳过它（无法最小化的窗口
                    // 要么被隐藏——已被子类拦截，要么原地不动）
                    let st = GetWindowLongPtrW(hwnd, GWL_STYLE) as isize;
                    if st & (WS_MINIMIZEBOX.0 as isize) != 0 {
                        SetWindowLongPtrW(
                            hwnd,
                            GWL_STYLE,
                            (st & !(WS_MINIMIZEBOX.0 as isize)) as _,
                        );
                        crate::log_msg(&format!(
                            "show-desktop-immune: 已移除最小化按钮样式 label={}",
                            win.label()
                        ));
                    }
                } else {
                    set_no_minimize(hwnd, false);
                    // 恢复最小化能力
                    let st = GetWindowLongPtrW(hwnd, GWL_STYLE) as isize;
                    if st & (WS_MINIMIZEBOX.0 as isize) == 0 {
                        SetWindowLongPtrW(
                            hwnd,
                            GWL_STYLE,
                            (st | (WS_MINIMIZEBOX.0 as isize)) as _,
                        );
                    }
                }
                // 保险：清掉旧版本遗留的桌面子窗口状态（WS_CHILD / 分层样式），
                // 恢复成普通顶层窗口，保证透明合成走可靠的顶层路径
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as isize;
                if style & (WS_CHILD.0 as isize) != 0 {
                    let _ = SetParent(hwnd, None);
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_STYLE,
                        ((style & !(WS_CHILD.0 as isize)) | (WS_POPUP.0 as isize)) as _,
                    );
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as isize;
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_EXSTYLE,
                        (ex & !(WS_EX_LAYERED.0 as isize)) as _,
                    );
                    let _ = SetWindowPos(
                        hwnd,
                        None,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    );
                }
            }
        }
    }
    let _ = win;
}

/// 判断窗口是否为「桌面常驻组件 / 分区」——显示桌面免疫只作用于这些，主窗口等不受影响。
pub fn is_desktop_widget(label: &str) -> bool {
    label.starts_with("box-")
        || matches!(label, "notes" | "stocks" | "huangli" | "poetry" | "huangdi" | "shanghan")
}

/// 把窗口从桌面父窗口上摘下来，恢复成独立的顶层窗口（**不动**防最小化子类过程）。
fn detach_from_desktop(win: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetParent, GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE, GWL_EXSTYLE, WS_CHILD,
            WS_POPUP, WS_EX_LAYERED, SetWindowPos, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE,
            SWP_NOACTIVATE,
        };
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                let _ = SetParent(hwnd, None);
                let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as isize;
                let new_style = (style & !(WS_CHILD.0 as isize)) | (WS_POPUP.0 as isize);
                SetWindowLongPtrW(hwnd, GWL_STYLE, new_style as _);
                // 摘掉 24H2+ 钉桌面时加的 WS_EX_LAYERED（未设置属性的分层窗口
                // 会被整个隐藏，恢复顶层窗口前必须清掉）
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as isize;
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !(WS_EX_LAYERED.0 as isize)) as _);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
                );
            }
        }
    }
    let _ = win;
}

/// 重启资源管理器**之前**调用：把所有组件/分区从桌面父窗口上摘下来。
///
/// 窗口挂成 explorer 的 `Progman` 子窗口后，explorer 退出时会销毁 `Progman`，
/// **子窗口会被父窗口连带销毁** —— 表现为「重启资源管理器后分区全没了」。
/// 所以必须先把它们恢复成独立顶层窗口，explorer 起来之后再挂回去。
pub fn detach_from_desktop_all(app: &tauri::AppHandle) {
    for w in app.webview_windows().values() {
        if is_desktop_widget(w.label()) {
            detach_from_desktop(&w);
        }
    }
}

/// 重启资源管理器**之后**调用：按当前设置重新挂回桌面。
pub fn attach_to_desktop_all(app: &tauri::AppHandle) {
    let keep = app
        .state::<Db>()
        .get_setting("app")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<crate::commands::AppSettings>(&s).ok())
        .map(|x| x.keep_on_show_desktop)
        .unwrap_or(true);
    for w in app.webview_windows().values() {
        if is_desktop_widget(w.label()) {
            apply_show_desktop_immune(&w, keep);
        }
    }
}

/// 收集"桌面窗口"：直接包含桌面图标视图（SHELLDLL_DefView）的顶层窗口。
/// 这一层才是真正压在壁纸上的东西，把它压到我们下面，我们就一定在桌面之上。
#[cfg(target_os = "windows")]
fn desktop_windows() -> Vec<windows::Win32::Foundation::HWND> {
    use windows::core::{w, BOOL};
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, FindWindowExW};

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = &mut *(lparam.0 as *mut Vec<HWND>);
        if let Ok(dv) = FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), None) {
            if !dv.is_invalid() {
                out.push(hwnd);
            }
        }
        BOOL(1)
    }

    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut out as *mut Vec<HWND> as isize));
    }
    out
}

/// 把分区面板沉到"桌面之上、其它所有窗口之下"。
///
/// Windows 没有可靠的 always-on-bottom：直接把窗口丢到 Z 序最底（HWND_BOTTOM），
/// 有可能掉到桌面窗口下面而彻底看不见。所以这里**顺序很关键**地做两步：
///   1) 先把自己压到 Z 序最底；
///   2) 再把桌面窗口（含桌面图标视图的那一层）也压到 Z 序最底 —— 它被顶到我们下面，
///      于是我们稳定处在"桌面之上、其它窗口之下"。
/// 这样别的程序或打开的文件窗口压过来时，面板永远不会盖住它们。
pub fn sink_to_desktop_level(win: &WebviewWindow) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, GetParent, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        };
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                // 已经挂成桌面窗口的子窗口（显示桌面免疫已启用）时，它本身就在桌面层，
                // 再对自己做 HWND_BOTTOM 会把它压到桌面图标层(SHELLDLL_DefView)之后，
                // 结果就是分区「看不见」。此时只需把桌面窗口压到底即可。
                let has_parent = GetParent(hwnd).map(|p| !p.is_invalid()).unwrap_or(false);
                if !has_parent {
                    let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
                }
                for desk in desktop_windows() {
                    let _ = SetWindowPos(desk, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
                }
            }
        }
    }
    let _ = win;
}

/// 窗口是否带 WS_EX_TOPMOST（展开中的饼型分区悬浮标记）。
/// 守护线程据此豁免：展开置顶是故意为之，不能被拽回桌面组。
#[cfg(target_os = "windows")]
fn is_topmost(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TOPMOST};
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as isize;
        ex & (WS_EX_TOPMOST.0 as isize) != 0
    }
}

/// 饼型分区展开态的「悬浮」：展开的文件必须浮在其它分区/窗口之上才点得到，
/// 展开瞬间把窗口抬到 TOPMOST（NOACTIVATE，不抢焦点、不触发沉回链）；
/// 收回时去掉置顶并归还桌面层，继续当安静的桌面挂件。
pub fn set_radial_floating(win: &WebviewWindow, float: bool) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, GWL_STYLE, SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE,
            SWP_NOMOVE, SWP_NOSIZE, WS_CHILD,
        };
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                // 「显示桌面免疫」会把分区挂成桌面子窗口：**子窗口没有 TOPMOST
                // band**，置顶无效、z 序只在桌面内部比较——展开文件会被同为
                // 子窗口的其它分区盖住（实测根因）。展开前必须先摘回顶层。
                let is_child = GetWindowLongPtrW(hwnd, GWL_STYLE) & (WS_CHILD.0 as isize) != 0;
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
                if float {
                    if is_child {
                        detach_from_desktop(win);
                    }
                    let ok = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, flags).is_ok();
                    // 置顶链路留痕：float / 挂载中 / SetWindowPos 结果，排查"没在顶层"用
                    crate::log_msg(&format!(
                        "radial-float: on label={} wasChild={is_child} setpos_ok={ok}",
                        win.label()
                    ));
                } else {
                    let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
                    sink_to_desktop_level(win);
                    // 原本挂着显示桌面免疫的分区：收回展开后挂回桌面，免疫不丢
                    if is_child {
                        let keep = win
                            .app_handle()
                            .state::<Db>()
                            .get_setting("app")
                            .ok()
                            .flatten()
                            .and_then(|s| serde_json::from_str::<crate::commands::AppSettings>(&s).ok())
                            .map(|x| x.keep_on_show_desktop)
                            .unwrap_or(true);
                        if keep {
                            apply_show_desktop_immune(win, true);
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (win, float);
}

/// 上一次"沉到桌面层"的时间（节流用）
static LAST_SINK: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// 读取「显示桌面时保持组件/分区可见」设置。
fn keep_on_show_desktop(app: &AppHandle) -> bool {
    app.state::<Db>()
        .get_setting("app")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<crate::commands::AppSettings>(&s).ok())
        .map(|x| x.keep_on_show_desktop)
        .unwrap_or(true)
}

/// 分区 / 组件获得焦点时的"归位"。
///
/// 先重新确认挂在桌面上：tao 在 `hide()/show()` 时会按它自己记录的 flags 重算窗口
/// 风格，可能把我们手动 `SetParent` 到 Progman 的父子关系弄丢；父窗口一旦丢失，
/// 再触发"沉到桌面层"就会被 `HWND_BOTTOM` 压到桌面图标层之下——表现为
/// 「分区凭空消失」。所以每次焦点归位都顺手把父子关系重新钉上（幂等、有节流），
/// 然后再沉到"桌面之上、其它窗口之下"。
pub fn sink_desktop_widget(win: &WebviewWindow, app: &AppHandle) {
    if keep_on_show_desktop(app) {
        apply_show_desktop_immune(win, true);
    }
    sink_to_desktop_level(win);
}

/// 焦点变化时要不要再做一次"沉到桌面层"。
///
/// [`sink_to_desktop_level`] 要枚举所有顶层窗口、再对桌面窗口逐个 `SetWindowPos`，
/// 而它挂在"窗口获得焦点"上 —— 连着点几下分区就会跑好几遍，是白白抖 CPU。
/// 300ms 内只做一次足够（迟一点点沉下去，视觉上无感）。
fn should_sink_again() -> bool {
    let Ok(mut last) = LAST_SINK.lock() else {
        return true;
    };
    if let Some(t) = *last {
        if t.elapsed() < Duration::from_millis(300) {
            return false;
        }
    }
    *last = Some(Instant::now());
    true
}

/// 拖动悬停事件的节流状态
static LAST_HOVER: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));

/// 要不要把这次"拖动经过"的落点转给前端。
///
/// 拖动过程中 Over 事件非常密集（鼠标每动一下就来一次），全部转给前端会刷爆 IPC。
/// 40ms 一次足够让"穿插到哪一格"的预览跟手。
pub(crate) fn should_emit_hover() -> bool {
    let Ok(mut last) = LAST_HOVER.lock() else {
        return true;
    };
    if let Some(t) = *last {
        if t.elapsed() < Duration::from_millis(40) {
            return false;
        }
    }
    *last = Some(Instant::now());
    true
}

/// 分区菜单窗口的 label
pub fn menu_label(box_id: i64) -> String {
    format!("boxmenu-{box_id}")
}

/// 本次 OLE 拖拽是否已经被某个分区窗口接住。
///
/// `SHDoDragDrop` 是模态循环，分区的 `Drop` 事件**在它返回之前**就会同步派发，
/// 所以拖拽结束后读这个标记是可靠的。有了它，源分区就不必再靠"文件还在不在原地"
/// 去猜落点是复制还是搬走 —— 那条路有竞态（分区那边的搬运跑在后台线程里，
/// 常常还没落盘检查就做完了）。
pub static TRANSFER_HANDLED: AtomicBool = AtomicBool::new(false);

/// 拖放时是否按着 Ctrl（且没按 Shift）—— 和资源管理器一致：Ctrl = 复制。
///
/// wry 的落点实现把返回给拖拽源的效果**写死成 DROPEFFECT_COPY** 且不看修饰键，
/// 所以想区分"移动 / 复制"只能自己读一次键盘状态。
pub fn modifier_copy() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
        let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };
        down(VK_CONTROL.0 as i32) && !down(VK_SHIFT.0 as i32)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// 把搬运结果告诉目标分区，由分区面板弹一条提示（成功也提示，失败给出原因）。
///
/// 接收 `Result` 是为了把「连落点文件夹都没解析出来」这种整体失败也变成一条可见提示，
/// 而不是在调用处 `return` 掉、用户什么都看不到。
pub fn emit_transfer_result(
    app: &AppHandle,
    box_id: i64,
    r: Result<engine::TransferResult, String>,
) {
    let r = r.unwrap_or_else(|e| engine::TransferResult {
        failed: 1,
        errors: vec![e],
        ..Default::default()
    });
    let Some(label) = app.try_state::<BoxWindows>().and_then(|b| b.label_for(box_id)) else {
        return;
    };
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.emit(
            "box-transfer-result",
            serde_json::json!({ "boxId": box_id, "result": r }),
        );
    }
}

/// 一批路径落进分区后的处理。
///
/// 落点事件（tao 的 `DragDrop::Drop`）和 [`crate::shelldrop`] 里自己实现的
/// `IDropTarget` 共用这一份，免得两套落点各自长出不同的行为。
///
/// `pos` 是**窗口内的逻辑坐标**，前端拿它算"该插到第几位"。
pub fn run_drop(app: &AppHandle, box_id: i64, paths: Vec<String>, pos: (f64, f64)) {
    // 立刻置位（同步），供 `start_file_drag` 判断"这次拖拽已经被某个分区接住了"。
    // 不置位的话，源分区会按"文件还在原地"误判成复制，还会把移动记录删掉。
    TRANSFER_HANDLED.store(true, Ordering::SeqCst);
    let db = match app.try_state::<Db>() {
        Some(d) => d.inner().clone(),
        None => return,
    };
    let app3 = app.clone();
    // 真正的搬运放到后台线程，避免阻塞窗口事件循环（主线程）
    std::thread::spawn(move || {
        // 落点文件夹：普通分区是分区自己的文件夹（对桌面隐藏）；
        // 快捷文件夹分区就是它链接的那个文件夹 —— 拖进来也应该真的放进去
        let dir = match engine::box_dir_for(&db, box_id) {
            Ok(d) => d,
            Err(e) => {
                // 关联文件夹没设 / 已失效：明确告诉用户，别让他对着"没反应"猜
                emit_transfer_result(&app3, box_id, Err(e));
                return;
            }
        };
        // 拖回自己这一格 → 不是搬运，而是"调整图标位置"：
        // 把落点坐标交给前端，由它算出该插到第几位，再记进分区配置。
        // 其余的才交给统一的搬运入口。
        let mut to_transfer: Vec<String> = Vec::new();
        for p in paths {
            let src = Path::new(&p);
            if src.parent() == Some(dir.as_path()) {
                if let Some(label) = app3
                    .try_state::<BoxWindows>()
                    .and_then(|b| b.label_for(box_id))
                {
                    let _ = app3.emit_to(
                        label,
                        "box-item-drop",
                        serde_json::json!({
                            "boxId": box_id,
                            "path": p,
                            "x": pos.0,
                            "y": pos.1,
                        }),
                    );
                }
                continue;
            }
            to_transfer.push(p);
        }
        if to_transfer.is_empty() {
            return;
        }
        // Ctrl（不带 Shift）= 复制，其余 = 移动，和资源管理器一致。
        // wry 的落点永远把效果报成 COPY，所以我们自己读键盘状态来定。
        let copy = modifier_copy();
        let res = engine::transfer_into_box(&db, box_id, &to_transfer, copy);
        // 源分区可能也变了（文件从那儿搬走了）→ 广播刷新，谁有变化谁重读
        crate::commands::notify_box_files_changed(&app3);
        emit_transfer_result(&app3, box_id, res);
    });
}

/// 松手时光标底下是哪个分区（排除 `exclude`）。
///
/// 兜底用：正常情况下分区窗口能自己收到 `DragDrop::Drop`，用不上这里；
/// 但落点通道万一没把事件交上来（窗口被挂成桌面子窗口、WebView 重建等），
/// 就靠它按窗口矩形判断"落到了哪个分区"，至少还能把文件搬过去。
/// 多个分区重叠时取面积最小的那个（小面板盖在大面板上更符合直觉）。
pub fn box_under_cursor(app: &AppHandle, exclude: i64) -> Option<i64> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return None;
        }
        box_at_point(app, pt.x, pt.y, exclude)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, exclude);
        None
    }
}

/// 屏幕点 `(x, y)` 下 **z 序最上** 的那个分区；命中 `exclude` 或该分区不可见时返回 `None`。
///
/// 为什么按 z 序、而不是"矩形包含 + 面积最小"（曾经的实现）：
/// 分区一旦重叠，重叠区里每个分区的 `GetWindowRect` 都会命中，此时选谁全看面积大小，
/// 跟用户眼睛看到的前后关系毫无关系 —— 结果就是明明指着被压在下面的那个分区，
/// 文件却落到了它**背后**那个分区（用户反馈："拖动释放的位置背后也有一个分区，
/// 那个分区也会把这个文件收下"）。`WindowFromPoint` 返回的正是"最上层、包含该点的
/// 那个窗口"，与视觉一致，落在它上面的分区才是用户真正指着的那一个。
pub fn box_at_point(app: &AppHandle, x: i32, y: i32, exclude: i64) -> Option<i64> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
        let hit = unsafe { WindowFromPoint(POINT { x, y }) };
        if hit.is_invalid() {
            return None;
        }
        // WebView2 的客户区被它自己的子窗口整个盖住，命中几乎总是落在子窗口上 → 取顶层祖先
        let root = unsafe { GetAncestor(hit, GA_ROOT) };
        let root = if root.is_invalid() { hit } else { root };
        let bw = app.state::<BoxWindows>();
        for id in bw.ids() {
            let Some(label) = bw.label_for(id) else { continue };
            let Some(win) = app.get_webview_window(&label) else { continue };
            let Ok(h) = win.hwnd() else { continue };
            if h.0 as isize != root.0 as isize {
                continue;
            }
            // 命中的就是这个分区：被排除、或自己不可见 → 都算"没有落点"，
            // 不能继续往下找 —— 用户指着谁就是谁，指着一个不接收的分区就该什么也不发生。
            if id == exclude || !win.is_visible().unwrap_or(false) {
                return None;
            }
            return Some(id);
        }
        None
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, x, y, exclude);
        None
    }
}

/// 当前光标的屏幕坐标。tao 的拖放事件只给"窗口内逻辑坐标"，
/// 而判定"落点该归哪个分区"必须用屏幕坐标（见 [`box_at_point`]）。
pub fn cursor_screen() -> (i32, i32) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) }.is_ok() {
            return (pt.x, pt.y);
        }
    }
    (-1, -1)
}

/// 诊断用：屏幕点上最上层窗口的类名与句柄。
///
/// 落点被丢弃时把它写进日志，一眼就能看出"是谁挡在分区前面"（别的程序窗口、
/// 桌面、还是我们自己的某个窗口），不必再靠猜。
pub fn window_desc_at(x: i32, y: i32) -> String {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetAncestor, GetClassNameW, WindowFromPoint, GA_ROOT,
        };
        unsafe {
            let hit = WindowFromPoint(POINT { x, y });
            if hit.is_invalid() {
                return "光标下没有窗口".to_string();
            }
            let root = {
                let r = GetAncestor(hit, GA_ROOT);
                if r.is_invalid() {
                    hit
                } else {
                    r
                }
            };
            let mut buf = [0u16; 256];
            let n = GetClassNameW(root, &mut buf).max(0) as usize;
            format!(
                "光标下是其它窗口 class={} hwnd={:?}",
                String::from_utf16_lossy(&buf[..n]),
                root.0
            )
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (x, y);
        String::new()
    }
}

/// 把菜单窗口摆到分区右上角外侧，并按内容高度定尺寸。
///
/// 为什么菜单要单独开窗口：菜单画在分区窗口里的话会被窗口边界裁掉 —— 分区越小越
/// 看不全；而且分区现在被"沉到桌面层"，画在里面的菜单还会被其它程序盖住。
/// 独立置顶小窗口把这两个问题一起解决。
pub fn place_box_menu(
    app: &AppHandle,
    box_id: i64,
    menu: &WebviewWindow,
    height: f64,
) -> Result<(), String> {
    let box_label = app
        .state::<BoxWindows>()
        .label_for(box_id)
        .ok_or("分区窗口不存在")?;
    let box_win = app.get_webview_window(&box_label).ok_or("分区窗口不存在")?;

    let scale = box_win.scale_factor().unwrap_or(1.0);
    let zoom = crate::ui_zoom(app);
    let pos = box_win.outer_position().map_err(|e| e.to_string())?;
    let size = box_win.outer_size().map_err(|e| e.to_string())?;

    // 菜单宽度（CSS 像素），与 CSS 里 `.box-win-menu` 的宽度保持一致。
    // 物理像素 = CSS × 显示器缩放 × 界面缩放（WebView zoom 后 CSS→物理多乘一次）
    let menu_w = 206.0_f64;
    let w = (menu_w * scale * zoom).round().max(1.0) as i32;
    let h = (height * scale * zoom).round().max(1.0) as i32;

    // 右上角对齐分区右上角，纵向让开标题栏（标题栏 CSS 高 34，同样受 zoom 影响）
    let mut x = pos.x + size.width as i32 - w;
    let mut y = pos.y + (34.0 * scale * zoom).round() as i32;

    // 贴边修正：别跑出屏幕（否则菜单就"看不全"了）
    if let Ok(Some(mon)) = box_win.current_monitor() {
        let mp = mon.position();
        let ms = mon.size();
        let left = mp.x;
        let top = mp.y;
        let right = mp.x + ms.width as i32;
        let bottom = mp.y + ms.height as i32;
        if y + h > bottom {
            y = (bottom - h).max(top);
        }
        if x + w > right {
            x = right - w;
        }
        x = x.max(left);
        y = y.max(top);
    }

    // 菜单窗口内容也按界面缩放渲染，与上面的尺寸换算公式保持一致
    let _ = menu.set_zoom(zoom);
    menu.set_size(PhysicalSize::new(w as u32, h as u32))
        .map_err(|e| e.to_string())?;
    menu.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 分区窗口落点：保证每一个面板都落在用户看得见的地方
// ---------------------------------------------------------------------------

/// 矩形 (x, y, w, h)，物理像素
type Rect = (i32, i32, i32, i32);

/// 一块显示器：完整范围 + 工作区（已去掉任务栏）+ 硬件身份
#[derive(Clone)]
struct MonInfo {
    monitor: Rect,
    work: Rect,
    /// 显示输出的 PnP 实例路径（如 `DELA0A1\5&10a58c63&0&UID4353`）。
    ///
    /// 这是分区"属于哪块屏"的锚点。不能用窗口系统给的 `\\.\DISPLAYn`：
    /// 那个编号随启动顺序 / 拔插重排（这次启动 DISPLAY2 是那台竖屏，
    /// 下次启动就可能是 DISPLAY1），当身份用必然串屏。
    /// PnP 实例路径由显示器型号 + 接口实例构成，同一台机器同一接口下跨重启稳定。
    key: String,
}

/// 枚举所有显示器。
///
/// 刻意用纯 GDI 而不是 tauri 的窗口 / 显示器接口：启动时这段代码跑在**主线程**上，
/// 而窗口类接口要往主线程投递消息再等结果，主线程自己调它有死锁风险。
/// GDI 这几个调用是无状态的，任何线程都能安全用。
#[cfg(target_os = "windows")]
fn all_monitors() -> Vec<MonInfo> {
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, DISPLAY_DEVICEW, HDC, HMONITOR,
        MONITORINFO, MONITORINFOEXW,
    };

    /// 从显示器设备名（`\\.\DISPLAY2`）查显示输出的 PnP 实例路径。
    ///
    /// GetMonitorInfoW 只给设备名，那是会重排的；EnumDisplayDevicesW 的
    /// `DeviceID` 才是"这台显示器插在这个接口上"的稳定身份（`DELA0A1\5&…&UID4353`）。
    /// 拿不到（驱动异常等）就退回设备名 —— 退化的身份总比没有强，
    /// 但这种 key 跨重启可能串屏，属兜底行为。
    unsafe extern "system" fn collect(
        hmon: HMONITOR,
        _hdc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let out = unsafe { &mut *(data.0 as *mut Vec<MonInfo>) };
        let mut miex = MONITORINFOEXW::default();
        miex.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // GetMonitorInfoW 按 cbSize 识别实际类型，传 MONITORINFOEXW 能拿到 szDevice
        if unsafe {
            GetMonitorInfoW(
                hmon,
                &mut miex as *mut MONITORINFOEXW as *mut MONITORINFO,
            )
            .as_bool()
        } {
            let device_name: Vec<u16> = miex
                .szDevice
                .iter()
                .take_while(|&&c| c != 0)
                .copied()
                .collect();
            let mut key = String::new();
            if !device_name.is_empty() {
                let mut dd = DISPLAY_DEVICEW::default();
                dd.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
                let ok = unsafe {
                    EnumDisplayDevicesW(
                        PCWSTR(device_name.as_ptr()),
                        0,
                        &mut dd,
                        0,
                    )
                    .as_bool()
                };
                if ok {
                    let id: String = String::from_utf16_lossy(
                        &dd.DeviceID.iter().take_while(|&&c| c != 0).copied().collect::<Vec<_>>(),
                    );
                    // DeviceID 形如 `DELA0A1\5&10a58c63&0&UID4353`（PnP 实例路径，稳定）
                    if !id.trim().is_empty() {
                        key = id.trim().to_string();
                    }
                }
            }
            if key.is_empty() {
                // 兜底：设备名（\\.\DISPLAYn）。会重排，但同一次会话内够用
                key = String::from_utf16_lossy(&device_name);
            }
            out.push(MonInfo {
                monitor: (
                    miex.monitorInfo.rcMonitor.left,
                    miex.monitorInfo.rcMonitor.top,
                    miex.monitorInfo.rcMonitor.right - miex.monitorInfo.rcMonitor.left,
                    miex.monitorInfo.rcMonitor.bottom - miex.monitorInfo.rcMonitor.top,
                ),
                work: (
                    miex.monitorInfo.rcWork.left,
                    miex.monitorInfo.rcWork.top,
                    miex.monitorInfo.rcWork.right - miex.monitorInfo.rcWork.left,
                    miex.monitorInfo.rcWork.bottom - miex.monitorInfo.rcWork.top,
                ),
                key,
            });
        }
        BOOL(1) // 非 0 = 继续枚举
    }

    let mut out: Vec<MonInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut out as *mut Vec<MonInfo> as isize),
        );
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn all_monitors() -> Vec<MonInfo> {
    Vec::new()
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// 两个矩形的交叠宽高（不相交时为 0）
fn overlap_size(a: Rect, b: Rect) -> (i32, i32) {
    let ox = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
    let oy = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
    (ox.max(0), oy.max(0))
}

/// 主显示器的工作区。
/// Windows 下主屏左上角固定是 (0,0)，所以包含 (0,0) 的那块就是主屏。
fn primary_work_area() -> Rect {
    let mons = all_monitors();
    mons.iter()
        .find(|m| rects_overlap(m.monitor, (0, 0, 1, 1)))
        .or_else(|| mons.first())
        .map(|m| m.work)
        .unwrap_or((0, 0, 1920, 1040))
}

/// 用户能不能**够得着**这个窗口？
///
/// 判据是"和某块显示器的交叠够不够大"：分区面板没有系统标题栏，拖动和右键菜单都要
/// 点在面板本身有内容的那一块上 —— 只露出一条边（不到 60×20 像素）等于看不见。
fn rect_reachable(x: i32, y: i32, w: i32, h: i32) -> bool {
    all_monitors().iter().any(|m| {
        let (ox, oy) = overlap_size((x, y, w, h), m.monitor);
        ox >= 60 && oy >= 20
    })
}

/// 和某块显示器有没有任意交叠（哪怕只露出一角）
fn overlaps_any_monitor(x: i32, y: i32, w: i32, h: i32) -> bool {
    all_monitors()
        .iter()
        .any(|m| rects_overlap((x, y, w, h), m.monitor))
}

/// 点 (x,y) 所在显示器的工作区；不在任何显示器上时取交叠最大的那块（也就是离它最近的）。
fn work_area_at(x: i32, y: i32) -> Rect {
    let mons = all_monitors();
    if let Some(m) = mons.iter().find(|m| {
        let (ox, oy) = overlap_size((x, y, 1, 1), m.monitor);
        ox > 0 && oy > 0
    }) {
        return m.work;
    }
    mons.iter()
        .max_by_key(|m| {
            let (ox, oy) = overlap_size((x, y, 1, 1), m.monitor);
            ox * oy
        })
        .map(|m| m.work)
        .unwrap_or((0, 0, 1920, 1040))
}

/// 点 (x,y) 所在的那块显示器（含硬件身份）。找不着返回 None。
fn monitor_at_point(x: i32, y: i32) -> Option<MonInfo> {
    all_monitors().into_iter().find(|m| {
        let (ox, oy) = overlap_size((x, y, 1, 1), m.monitor);
        ox > 0 && oy > 0
    })
}

/// 窗口在所属屏工作区内的**比例位置**：0 = 贴左/上，1 = 贴右/下。
///
/// 存比例而不是存"属于第几块屏 + 像素偏移"，是为了同时扛住两类变化：
/// 分辨率改了 → 比例不变，分区回到同一台屏上近似原来的位置；
/// 多屏排列互换（A↔B）→ 分区跟着屏幕的硬件身份走，不会串到别的屏。
fn rel_pos(wa: Rect, x: i32, y: i32, w: i32, h: i32) -> [f64; 2] {
    let dx = (wa.2 - w).max(1) as f64;
    let dy = (wa.3 - h).max(1) as f64;
    [
        ((x - wa.0) as f64 / dx).clamp(0.0, 1.0),
        ((y - wa.1) as f64 / dy).clamp(0.0, 1.0),
    ]
}

/// 比例位置 → 该屏工作区里的绝对位置（窗口自身尺寸不变）
fn abs_from_rel(wa: Rect, rel: [f64; 2], w: i32, h: i32) -> (i32, i32) {
    let dx = (wa.2 - w).max(0) as f64;
    let dy = (wa.3 - h).max(0) as f64;
    (
        wa.0 + (dx * rel[0].clamp(0.0, 1.0)).round() as i32,
        wa.1 + (dy * rel[1].clamp(0.0, 1.0)).round() as i32,
    )
}

/// 把窗口夹进某块工作区（窗口比工作区还大时就贴左上角）。
fn clamp_into(wa: Rect, x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    let (left, top, ww, hh) = wa;
    let (right, bottom) = (left + ww, top + hh);
    (
        x.min((right - w).max(left)).max(left),
        y.min((bottom - h).max(top)).max(top),
    )
}

/// 在工作区里找一个不压住已有分区的空位（从左到右、从上到下阶梯式铺开）。
fn free_slot_in(db: &Db, box_id: i64, wa: Rect, w: i32, h: i32) -> (i32, i32) {
    let (left, top, ww, hh) = wa;
    let mut taken: Vec<Rect> = Vec::new();
    if let Ok(cfgs) = db.get_box_windows_cfg() {
        for c in cfgs {
            if c.id == box_id {
                continue;
            }
            // 屏幕外的配置不算占位 —— 它们自己也在等着被重新安置，
            // 算进去只会把新分区挤到更奇怪的角落。
            if !rect_reachable(c.x, c.y, c.w, c.h) {
                continue;
            }
            taken.push((c.x, c.y, c.w, c.h));
        }
    }
    let margin = 40;
    let step_x = w + 16;
    let step_y = h + 16;
    let cols = ((ww - margin * 2).max(step_x) / step_x).max(1);
    let rows = ((hh - margin * 2).max(step_y) / step_y).max(1);
    // 第一趟找完全空着的位置；找不到就退而求其次，挑"被压得最少"的那个 ——
    // 宁可轻微叠在某个分区上，也绝不能把新分区藏到屏幕外面去。
    let mut best: Option<(i64, i32, i32)> = None;
    for r in 0..rows {
        for c in 0..cols {
            let x = left + margin + c * step_x;
            let y = top + margin + r * step_y;
            if x + w > left + ww || y + h > top + hh {
                continue;
            }
            let area: i64 = taken
                .iter()
                .map(|t| {
                    let (ox, oy) = overlap_size((x, y, w, h), *t);
                    ox as i64 * oy as i64
                })
                .sum();
            if area == 0 {
                return (x, y);
            }
            if best.map_or(true, |(a, _, _)| area < a) {
                best = Some((area, x, y));
            }
        }
    }
    if let Some((_, x, y)) = best {
        return (x, y);
    }
    // 连一个格子都放不下（工作区比分区本身还小）→ 贴工作区左上角
    clamp_into(wa, left, top, w, h)
}

/// 决定一个分区窗口最终摆在哪儿，并返回"位置是否被修正过"（修正过就要回写配置）。
///
/// 这一步是必需的，因为历史上的默认落点是按 `box_id` 递推的：
/// `(60 + id%3*330, 60 + id/3*250)`。分区 id 只增不减（删掉再新建，id 更大），
/// 位置就一路往屏幕下方排 —— 到 id 60 上下 y 已经 5000+，整块跑到屏幕外面。
/// 表现就是用户说的"提示有新建分区，但完全看不见新面板"，而且那个坐标还会被落盘，
/// 重启之后依旧看不见。
/// 分区窗口的尺寸下限：再小就没法操作了（标题栏按钮靠 CSS 自适应让位）。
const MIN_BOX_W: i32 = 140;
const MIN_BOX_H: i32 = 100;
/// 新分区 / 几何配置不可用时的默认窗口尺寸。
const DEFAULT_BOX_W: i32 = 330;
const DEFAULT_BOX_H: i32 = 250;
/// 胶囊模式的窗口尺寸（**逻辑**像素；建窗 / 收起时乘 scale factor 变物理像素）。
const CAPSULE_W: f64 = 150.0;
const CAPSULE_H: f64 = 34.0;
/// 饼型菜单圆钮的窗口尺寸（逻辑像素）。
const RADIAL_BTN: f64 = 54.0;

/// 胶囊尺寸（物理像素）。
fn capsule_size(sf: f64) -> (i32, i32) {
    (
        (CAPSULE_W * sf).round().max(40.0) as i32,
        (CAPSULE_H * sf).round().max(24.0) as i32,
    )
}

/// 饼型菜单圆钮尺寸（物理像素）。
fn radial_btn_size(sf: f64) -> (i32, i32) {
    (
        (RADIAL_BTN * sf).round().max(40.0) as i32,
        (RADIAL_BTN * sf).round().max(40.0) as i32,
    )
}

/// 该分区是不是饼型菜单（radial_style 非空）。
fn is_radial(cfg: Option<&BoxWindowCfg>) -> bool {
    cfg.map(|c| !c.radial_style.is_empty()).unwrap_or(false)
}

/// 由配置决定窗口几何：`(w, h, visible, locked, 配置是否不可用)`。
///
/// 配置**尺寸不可用**有两种情况，都按"没配置过"处理、用默认尺寸：
/// - 没有配置（新分区）；
/// - 有配置但 `w`/`h` 小于下限 —— 备份文件里缺这两个字段时，serde 会补成 0，
///   于是窗口被夹到 140×100 落在 (0,0)，变成一个卡在左上角、锁定后谁也动不了的碎片。
///
/// `visible` / `locked` 即使尺寸不可用也照旧沿用配置值：这两个是用户的意图，
/// 不该因为几何坏掉就被重置。
fn box_size_from_cfg(cfg: Option<&BoxWindowCfg>) -> (i32, i32, bool, bool, bool) {
    let degenerate = cfg
        .map(|c| c.w < MIN_BOX_W || c.h < MIN_BOX_H)
        .unwrap_or(true);
    match cfg {
        Some(c) if !degenerate => (c.w, c.h, c.visible, c.locked, false),
        other => (
            DEFAULT_BOX_W,
            DEFAULT_BOX_H,
            other.map(|c| c.visible).unwrap_or(true),
            other.map(|c| c.locked).unwrap_or(false),
            degenerate,
        ),
    }
}

fn resolve_box_geometry(
    db: &Db,
    box_id: i64,
    w: i32,
    h: i32,
    cfg: Option<&BoxWindowCfg>,
) -> (i32, i32, bool) {
    let (w, h) = (w.max(MIN_BOX_W), h.max(MIN_BOX_H));
    if let Some(c) = cfg {
        // 分区绑定了显示器（多屏场景）：按硬件身份找回它归属的那块屏，
        // 用比例位置落位 —— 分辨率改了 / 显示器排列换了，分区都跟着自己的屏走。
        // 找不到（这台屏现在没接）才落到下面的通用兜底。
        if !c.monitor_key.is_empty() {
            if let Some(rel) = c.rel {
                if let Some(mon) = all_monitors().iter().find(|m| m.key == c.monitor_key) {
                    let (x, y) = abs_from_rel(mon.work, rel, w, h);
                    let (x, y) = clamp_into(mon.work, x, y, w, h);
                    return (x, y, false);
                }
                // 归属的屏不在了：继续用绝对坐标走兜底（大概率整块在外 → 重新安置）
            }
        }
        // 用户自己摆好的位置优先，原样保留
        if rect_reachable(c.x, c.y, w, h) {
            return (c.x, c.y, false);
        }
        if overlaps_any_monitor(c.x, c.y, w, h) {
            // 只出了半截 → 夹回它所在那块屏的工作区
            let wa = work_area_at(c.x + w / 2, c.y + h / 2);
            let (nx, ny) = clamp_into(wa, c.x, c.y, w, h);
            return (nx, ny, true);
        }
        // 整块都在屏幕外 → 重新给它安置一个空位
        let (nx, ny) = free_slot_in(db, box_id, primary_work_area(), w, h);
        return (nx, ny, true);
    }
    // 新建分区：直接分配一个空位
    let (x, y) = free_slot_in(db, box_id, primary_work_area(), w, h);
    (x, y, false)
}

/// 创建（或复用）一个分区窗口
pub fn open_box_window(
    app: &AppHandle,
    db: &Db,
    box_id: i64,
    name: &str,
    color: &str,
) -> Result<(), String> {
    let cfg = db.get_box_window_cfg(box_id)?;
    open_box_window_with_cfg(app, db, box_id, name, color, cfg, false).map(|_| ())
}

/// 创建（或复用）一个分区窗口（配置由调用方预加载，避免逐个分区重复解析设置 JSON）。
///
/// `defer_show = true` 时窗口先以隐藏状态创建，若该分区本应可见则把窗口返回给调用方，
/// 由调用方在全部分区创建完毕后**一次性显示**——消除启动时逐个弹出的闪烁。
fn open_box_window_with_cfg(
    app: &AppHandle,
    db: &Db,
    box_id: i64,
    name: &str,
    _color: &str,
    cfg: Option<BoxWindowCfg>,
    defer_show: bool,
) -> Result<Option<tauri::WebviewWindow>, String> {
    let label = format!("box-{box_id}");
    if app.get_webview_window(&label).is_some() {
        return Ok(None);
    }
    // 注意：icon_size / bg_color / bg_opacity 不在这里用——
    // 这些是分区内容（由前端渲染），窗口本身只关心几何与可见性。
    // 配置里的几何是否可用：备份文件缺几何字段时 `w`/`h` 会反序列化成 0，
    // 这种配置建出来的窗口会被夹到 140×100 并落在 (0,0) —— 一个卡在屏幕左上角的
    // "碎片窗口"。它若还处于锁定状态，既不能拖也不能改大小，而标题栏的解锁按钮
    // 早已被挤出可视区，用户就彻底动不了它（"等于卡死在那"）。
    // 所以尺寸不合法时按"没配置过"处理：默认尺寸 + 重新安置到工作区空位。
    //
    // 胶囊模式例外：收起的分区本来就只有胶囊那么大（w/h 远小于下限是**有意为之**），
    // 不能走上面的"退化兜底"——否则每次重启都会把胶囊放大成默认尺寸。
    // 此时按胶囊尺寸建窗（下面建好后按 scale factor 校正物理尺寸），
    // 展开几何从 expanded_geom 恢复，cfg.w/h 不参与。
    let collapsed = cfg.as_ref().map(|c| c.collapsed).unwrap_or(false);
    let radial = is_radial(cfg.as_ref());
    // 逻辑像素估算值（建窗后按真实 scale factor 校正）：
    // 胶囊 = 胶囊尺寸；饼型菜单 = 圆钮尺寸（它永远处于收起语义）
    let (w, h, visible, locked, degenerate) = if collapsed {
        let (lw, lh) = if radial { (RADIAL_BTN, RADIAL_BTN) } else { (CAPSULE_W, CAPSULE_H) };
        (
            lw as i32,
            lh as i32,
            cfg.as_ref().map(|c| c.visible).unwrap_or(true),
            cfg.as_ref().map(|c| c.locked).unwrap_or(false),
            false,
        )
    } else {
        box_size_from_cfg(cfg.as_ref())
    };
    if degenerate && cfg.is_some() {
        crate::log_msg(&format!(
            "分区 {box_id} 的窗口尺寸配置不合法（小于 {MIN_BOX_W}×{MIN_BOX_H}），\
             已按默认尺寸 {DEFAULT_BOX_W}×{DEFAULT_BOX_H} 重新安置"
        ));
    }
    // 落点：尊重用户摆好的位置；整块跑到屏幕外（历史遗留）或新建的 → 重新安置到工作区空位
    let usable_cfg = if degenerate { None } else { cfg.as_ref() };
    let (x, y, repaired) = resolve_box_geometry(db, box_id, w, h, usable_cfg);
    if cfg.is_some() && (repaired || degenerate) {
        if repaired {
            crate::log_msg(&format!(
                "分区 {box_id} 的窗口原先在屏幕外（不可见），已重新安置到 ({x}, {y})"
            ));
        }
        // 立刻回写：不写的话下次启动还会按老坐标 / 坏尺寸把它藏回屏幕外
        let _ = db.update_box_window_cfg(box_id, |c| {
            c.x = x;
            c.y = y;
            if degenerate {
                c.w = w;
                c.h = h;
            }
        });
    }

    let url = WebviewUrl::App(format!("index.html?box={box_id}").into());
    // 一律先隐藏建窗：配置里存的是 outer_position / outer_size（物理像素），
    // 而 builder 的 position / inner_size 用逻辑像素，高 DPI 屏（125% / 150%）上
    // 两者不等，直接塞进去会让重启后的布局整体放大 / 平移 scale 倍，
    // 表现为「退出时和重开后的分区布局对不上」。建好后用物理像素校正一次，
    // 与股票 / 黄历 / 诗词组件同一套做法。
    let win = WebviewWindowBuilder::new(app, &label, url)
        .title(name)
        .inner_size(w as f64, h as f64)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        .visible(false)
        .build()
        .map_err(|e| format!("创建分区窗口失败: {e}"))?;
    // 锁定的分区连原生边框拉伸也要禁掉（前端手柄已隐藏，这里是窗口样式这一层）；
    // 胶囊同理 —— 胶囊不允许被原生拉伸，尺寸只在收起/展开时由代码控制
    if locked || collapsed {
        let _ = win.set_resizable(false);
    }
    let _ = win.set_position(PhysicalPosition::new(x, y));
    // 胶囊：按本机 scale factor 折算物理尺寸（builder 里塞的是逻辑像素估算值）。
    // 普通分区：配置存的就是物理像素，直接用（下限兜底防退化）。
    if collapsed {
        if let Ok(sf) = win.scale_factor() {
            let (cw, ch) = if radial { radial_btn_size(sf) } else { capsule_size(sf) };
            let _ = win.set_size(PhysicalSize::new(cw.max(1) as u32, ch.max(1) as u32));
        }
    } else {
        let _ = win.set_size(PhysicalSize::new(
            w.max(MIN_BOX_W) as u32,
            h.max(MIN_BOX_H) as u32,
        ));
    }
    // 应用内界面缩放：内容等比放大，图标网格 auto-fill 自动重排
    crate::apply_ui_zoom(&win);

    // 强制不在任务栏显示（Tauri 的 skip_taskbar 在透明无边框窗口上可能失效）
    force_no_taskbar(&win);
    // 沉到"桌面之上、其它窗口之下"：面板永远不盖住别的程序
    sink_to_desktop_level(&win);
    // 接管拖放落点：tao 内置那个只认 CF_HDROP，此电脑 / 回收站这类
    // 只有 shell 标识、没有文件路径的桌面图标就永远拖不进来。
    // 必须在窗口建好之后调用（早于 tao 注册会让它那边 assert 失败）。
    crate::shelldrop::install(app, &win, box_id);
    // 原生边缘拉伸的吸附（拉窗口边框走系统 resize 循环，前端 JS 接不到）
    crate::resize_snap::install(&win, &label);

    let bw = app.state::<BoxWindows>();
    bw.register(box_id, &label);

    // 移动/调整大小 → 节流持久化；关闭 → 隐藏（不销毁）；拖放 → 移动文件
    let win2 = win.clone();
    let app2 = app.clone();
    win.on_window_event(move |event| match event {
        WindowEvent::Moved(_) => persist_cfg(&win2, box_id, false),
        WindowEvent::Resized(_) => persist_cfg(&win2, box_id, false),
        // 被激活时 Windows 会把窗口浮到最上面，这里立刻把它沉回桌面层，
        // 保证"点一下面板"之后它也不会盖住别的程序（键盘输入仍然是通的）。
        // 做了节流：这一步要枚举所有顶层窗口再逐个 SetWindowPos，连点几下不值得跑好几遍。
        WindowEvent::Focused(true) => {
            if should_sink_again() {
                sink_desktop_widget(&win2, &app2);
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            persist_cfg(&win2, box_id, true);
            hide_win(&win2);
            // 分区收起来了，独立菜单窗口也一起收起，别留个孤儿浮在上面
            if let Some(m) = win2.app_handle().get_webview_window(&menu_label(box_id)) {
                let _ = m.hide();
            }
        }
        // 窗口被销毁（关闭分区 / 程序退出）时，把缓存的"最终几何"落盘：
        // 移动/调整有 500ms 节流，拖动的最后一帧常被丢掉，不补这一步重启后位置会回退。
        WindowEvent::Destroyed => {
            crate::shelldrop::uninstall(&win2);
            if let Some(bw) = app2.try_state::<BoxWindows>() {
                if let Some((x, y, w, h)) = bw.take_geom(box_id) {
                    let db = app2.state::<Db>();
                    let _ = db.update_box_window_cfg(box_id, |c| {
                        c.x = x;
                        c.y = y;
                        c.w = w;
                        c.h = h;
                    });
                }
            }
        }
        // 拖动经过本分区：把落点转给前端，让图标"跟着鼠标穿插"，松手前就能看到结果
        WindowEvent::DragDrop(tauri::DragDropEvent::Over { position }) => {
            if should_emit_hover() {
                let _ = win2.emit(
                    "box-item-hover",
                    serde_json::json!({ "boxId": box_id, "x": position.x, "y": position.y }),
                );
            }
        }
        // 拖出分区 → 告诉前端收起预览
        WindowEvent::DragDrop(tauri::DragDropEvent::Leave) => {
            let _ = win2.emit(
                "box-item-hover",
                serde_json::json!({ "boxId": box_id, "x": -1, "y": -1 }),
            );
        }
        WindowEvent::DragDrop(ev) => {
            if let tauri::DragDropEvent::Drop { paths, position } = ev {
                let paths: Vec<String> = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                let pos = (position.x, position.y);
                // 必须走统一准入闸门，不能直接 run_drop：WebView2 内建的拖放通道
                // （wry → 这里）和我们自己的 `IDropTarget` 会对**同一个物理落点**
                // 各报一次，两边都直接 run_drop 就会把同一批文件搬进两个分区。
                crate::shelldrop::route_drop(
                    &app2,
                    box_id,
                    paths,
                    pos,
                    cursor_screen(),
                    crate::shelldrop::DropVia::Tao,
                );
            }
        }
        _ => {}
    });

    // 批量创建：保持隐藏，交给调用方统一显示；单个创建：本应可见就直接显示
    if defer_show {
        Ok(visible.then_some(win))
    } else {
        if visible {
            win.show().map_err(|e| e.to_string())?;
        }
        Ok(None)
    }
}

/// 为数据库中所有分区创建窗口
pub fn open_all_box_windows(app: &AppHandle, db: &Db) -> Result<(), String> {
    // 一次查询取全部分区（含类型），避免逐个分区再查一次
    let list: Vec<(i64, String, String, String, i64)> = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, color, type, nested_in FROM boxes ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    // 布局配置一次性载入（每个分区单独读会重复解析同一份 JSON）
    let cfgs = db.get_box_windows_cfg().unwrap_or_default();
    // 桌面路径只需解析一次
    let desktop = engine::desktop_dir().ok();
    let mut to_show: Vec<tauri::WebviewWindow> = Vec::new();
    // 当前库里真实存在的分区 id：稍后用它清扫"幽灵窗口"（见 sweep_ghost_box_windows）
    let mut valid_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (id, name, color, box_type, nested_in) in list {
        valid_ids.insert(id);
        // 启动时刷新已有分区文件夹的隐藏属性（Windows 有时会把它重置掉），
        // 桌面只显示透明面板。
        // 注意用 refresh_box_dir_hidden 而不是 hide_dir：它只认"我们自己建的"
        // 分区文件夹，用户桌面上自己的同名文件夹绝不会被打成隐藏。
        if box_type != "folder" {
            if let Some(desk) = &desktop {
                engine::refresh_box_dir_hidden(&desk.join(&name));
            }
        }
        let cfg = cfgs.iter().find(|c| c.id == id).cloned();
        let win = open_box_window_with_cfg(app, db, id, &name, &color, cfg, true)?;
        // 嵌套的饼型分区：窗口照常创建（脚本活着，点条目能秒弹），
        // 但绝不进批量显示 —— 它的可见性只由 nested_in 决定。
        if nested_in == 0 {
            if let Some(w) = win {
                to_show.push(w);
            }
        }
    }
    // 全部分区窗口就绪后一次性显示：每个 WebView2 要几百毫秒初始化，
    // 逐个显示会让桌面"噼里啪啦"弹 N 次；批量显示只有一次视觉变化。
    // （嵌套饼型分区的窗口虽然建了，但 visible=false 不会进 to_show，
    //   点宿主分区里的条目时才按需显示 —— 见 open_nested_radial。）
    for w in to_show {
        let _ = w.show();
    }
    // 清扫上一轮"恢复备份"残留下来的幽灵窗口（库里已无对应分区）
    sweep_ghost_box_windows(app, &valid_ids);
    Ok(())
}

/// 销毁分区窗口（删除分区时）
pub fn destroy_box_window(app: &AppHandle, box_id: i64) {
    let label = app.state::<BoxWindows>().label_for(box_id);
    destroy_box_window_at(app, box_id, label.as_deref());
}

/// 销毁分区窗口（label 已知 / 注册表里已经没有时用）。
fn destroy_box_window_at(app: &AppHandle, box_id: i64, label: Option<&str>) {
    let bw = app.state::<BoxWindows>();
    // 先取消注册并删掉配置，再销毁窗口：反过来做的话，Destroyed 回调里"落盘最终几何"
    // 可能在这之后才执行，把刚删掉的配置又写回去，留下一条永远打不开的僵尸配置。
    bw.unregister(box_id);
    let db = app.state::<Db>();
    let _ = db.remove_box_window_cfg(box_id);
    if let Some(label) = label {
        if let Some(win) = app.get_webview_window(label) {
            // 卸载防最小化子类并释放句柄记录：HWND 数值会被系统复用，
            // 不清理的话新窗口拿到同一个数值时会被误判成"已安装"而静默失效。
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                set_no_minimize(hwnd, false);
            }
            let _ = win.destroy();
        }
    }
    // 分区的右键菜单是**独立的 WebView2 窗口**，不跟着一起销毁就会永久残留
    // （每删/合并一个分区泄漏一个隐藏的渲染进程）。
    if let Some(m) = app.get_webview_window(&menu_label(box_id)) {
        let _ = m.destroy();
    }
}

/// 清理"幽灵分区窗口"。
///
/// 背景（实测）：恢复备份 / 导入布局会把 `boxes` 表**整体重建**，分区 id 全部换新。
/// 旧分区的窗口如果没被关掉，就会继续留在桌面上，而库里已经没有任何记录能管到它 ——
/// 用户看到的是"多出来一块拖不动、改不了、也点不开的小面板"，而且它照样被桌面层守护
/// 保护着，越攒越多。这里做一次兜底清扫：凡是我们自己的、label 形如 `box-<id>` /
/// `boxmenu-<id>`、而 id 已不在库里的窗口，一律销毁。
///
/// 放在**所有分区窗口都建好之后**执行：此刻注册表是干净的，清扫不会误伤正在创建的窗口。
pub fn sweep_ghost_box_windows(app: &AppHandle, valid_ids: &std::collections::HashSet<i64>) {
    let mut ghosts: Vec<(String, i64)> = Vec::new();
    for (label, _) in app.webview_windows() {
        let rest = label
            .strip_prefix("boxmenu-")
            .or_else(|| label.strip_prefix("box-"));
        let Some(rest) = rest else { continue };
        let Ok(id) = rest.parse::<i64>() else { continue };
        if !valid_ids.contains(&id) {
            ghosts.push((label.clone(), id));
        }
    }
    if ghosts.is_empty() {
        return;
    }
    for (label, id) in ghosts {
        crate::log_msg(&format!(
            "清理幽灵分区窗口：{label}（分区 {id} 已不存在，窗口残留）"
        ));
        destroy_box_window_at(app, id, Some(&label));
    }
}

/// 显示/隐藏单个分区窗口
/// 显示 / 隐藏一个分区窗口（托盘菜单、主窗口的开关都会走这里）。
///
/// 嵌套中的饼型分区（nested_in != 0）不可见性由嵌套关系决定：显示它会把圆钮
/// 从宿主列表里"漏"到桌面上，所以显示请求直接忽略（隐藏请求无害，放行）。
pub fn set_box_visible(app: &AppHandle, box_id: i64, visible: bool) -> Result<(), String> {
    let db = app.state::<Db>();
    if visible {
        let nested: i64 = {
            let conn = db.conn.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT nested_in FROM boxes WHERE id = ?1",
                rusqlite::params![box_id],
                |r| r.get(0),
            )
            .unwrap_or(0)
        };
        if nested != 0 {
            crate::log_msg(&format!("分区 #{box_id} 嵌套在宿主分区里，忽略显示请求"));
            return Ok(());
        }
    }
    let bw = app.state::<BoxWindows>();
    let label = bw
        .label_for(box_id)
        .ok_or_else(|| "分区窗口不存在".to_string())?;
    if let Some(win) = app.get_webview_window(&label) {
        if visible {
            // 只显示、不抢焦点：分区钉在桌面层，setFocus 会把它抬到最顶层，
            // 随后又被守护逻辑沉回去 —— 白闪一次
            win.show().map_err(|e| e.to_string())?;
        } else {
            hide_win(&win);
        }
        persist_cfg(&win, box_id, true);
    }
    Ok(())
}

/// 嵌套饼型分区的窗口隐藏 / 显示。
///
/// **不写 cfg.visible**：嵌套分区的可见性完全由 boxes.nested_in 决定 ——
/// 启动时嵌套的一律不显示，退出落盘的 visible 值对它没有意义；
/// 若照常落盘 visible=false，解嵌后重启分区就再也不出现了。
pub fn set_radial_window_hidden(app: &AppHandle, box_id: i64, hidden: bool) -> Result<(), String> {
    let bw = app.state::<BoxWindows>();
    let label = bw
        .label_for(box_id)
        .ok_or_else(|| "分区窗口不存在".to_string())?;
    if let Some(win) = app.get_webview_window(&label) {
        if hidden {
            hide_win(&win);
        } else {
            win.show().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 打开一个嵌套的饼型分区（用户在宿主分区里点了它的圆钮条目）。
///
/// 圆钮窗口此刻是隐藏的：确保窗口存在（启动时嵌套分区也照常建窗，这里只是兜底），
/// 把圆钮中心挪到点击位置（夹回工作区），显示后发 `nested-radial-open`，
/// 由它自己的前端弹出展开环。窗口已可见时不重复挪位（展开中挪动会
/// 破坏"圆钮钉死不动"的约束），只补发一次展开事件。
pub fn open_nested_radial(app: &AppHandle, box_id: i64, cx: i32, cy: i32) -> Result<(), String> {
    let db = app.state::<Db>();
    let label = format!("box-{box_id}");
    let already_visible = app
        .get_webview_window(&label)
        .map(|w| w.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if !already_visible {
        let win = match app.get_webview_window(&label) {
            Some(w) => w,
            None => {
                // 窗口不在（异常兜底）：查元信息补建一个隐藏窗
                let (name, color): (String, String) = {
                    let conn = db.conn.lock().map_err(|e| e.to_string())?;
                    conn.query_row(
                        "SELECT name, color FROM boxes WHERE id = ?1",
                        rusqlite::params![box_id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .map_err(|_| "分区不存在".to_string())?
                };
                let cfg = db.get_box_window_cfg(box_id)?;
                open_box_window_with_cfg(app, &db, box_id, &name, &color, cfg, true)?;
                app.get_webview_window(&label)
                    .ok_or_else(|| "创建分区窗口失败".to_string())?
            }
        };
        let size = win.outer_size().map_err(|e| e.to_string())?;
        let (w, h) = (size.width.max(1) as i32, size.height.max(1) as i32);
        let wa = work_area_at(cx, cy);
        let (nx, ny) = clamp_into(wa, cx - w / 2, cy - h / 2, w, h);
        let _ = win.set_position(PhysicalPosition::new(nx, ny));
        win.show().map_err(|e| e.to_string())?;
    }
    let _ = app.emit_to(&label, "nested-radial-open", box_id);
    Ok(())
}

/// 程序退出前把所有分区窗口的当前几何强制落盘。
///
/// 移动/调整大小的持久化有 500ms 节流，拖动刚结束就退出时最后一帧可能还没写库；
/// 且 `app.exit(0)` 退出时窗口的 Destroyed 事件不保证触发（缓存几何来不及落盘）。
/// 退出路径显式调用这里，保证重启后的布局和退出那一刻完全一致。
pub fn persist_all_geometry(app: &AppHandle) {
    let Some(bw) = app.try_state::<BoxWindows>() else { return };
    for id in bw.ids() {
        if let Some(label) = bw.label_for(id) {
            if let Some(win) = app.get_webview_window(&label) {
                persist_cfg(&win, id, true);
            }
        }
    }
}

/// 收起 / 展开分区（胶囊模式）。
///
/// 收起：把当前几何存进 `expanded_geom`，窗口缩成胶囊并禁掉原生拉伸。
/// 展开：按 `expanded_geom` 恢复几何（缺失时退回 cfg 的 x/y/w/h）。
/// 已是目标状态时直接返回成功（幂等）——否则"重复收起"会把胶囊尺寸
/// 当成展开几何存进去，展开后就再也回不到原来的大小了。
///
/// 悬停临时展开不走这里：那只是前端把窗口临时放大，`collapsed` 标记不动，
/// 关闭 / 重启后仍按胶囊恢复（见 BoxWindow 前端的 doHoverExpand）。
pub fn set_box_collapsed(app: &AppHandle, box_id: i64, collapsed: bool) -> Result<(), String> {
    let bw = app.state::<BoxWindows>();
    let label = bw
        .label_for(box_id)
        .ok_or_else(|| "分区窗口不存在".to_string())?;
    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| "分区窗口不存在".to_string())?;
    let db = app.state::<Db>();
    let cfg = db
        .get_box_window_cfg(box_id)?
        .unwrap_or_else(|| BoxWindowCfg::new(box_id));

    // 饼型菜单分区没有"永久收起/展开"语义：平时永远是圆钮，单击弹出只是
    // 前端临时放大窗口。菜单里的「收起/展开」对它不做任何事，避免把
    // 圆钮尺寸配置写成胶囊或展开几何。
    if !cfg.radial_style.is_empty() {
        return Ok(());
    }

    if collapsed {
        if cfg.collapsed {
            return Ok(());
        }
        let pos = win.outer_position().map_err(|e| e.to_string())?;
        let size = win.outer_size().map_err(|e| e.to_string())?;
        let sf = win.scale_factor().unwrap_or(1.0);
        let (cw, ch) = capsule_size(sf);
        db.update_box_window_cfg(box_id, |c| {
            c.collapsed = true;
            c.expanded_geom = Some([pos.x, pos.y, size.width as i32, size.height as i32]);
            c.w = cw;
            c.h = ch;
        })?;
        let _ = win.set_resizable(false);
        // 只缩不挪：胶囊留在原分区的左上角，用户对"它在哪"的预期不变
        let _ = win.set_size(PhysicalSize::new(cw.max(1) as u32, ch.max(1) as u32));
        persist_cfg(&win, box_id, true);
    } else {
        if !cfg.collapsed {
            return Ok(());
        }
        // 展开几何：优先 expanded_geom；没有（历史遗留数据）就退回 cfg 的 x/y/w/h
        let (gx, gy, gw, gh) = match cfg.expanded_geom {
            Some([x, y, w, h]) => (x, y, w, h),
            None => (cfg.x, cfg.y, cfg.w, cfg.h),
        };
        let (nw, nh) = (gw.max(MIN_BOX_W), gh.max(MIN_BOX_H));
        // 恢复前把位置夹回工作区：收起期间显示器可能被拔掉 / 改过分辨率，
        // 直接还原会把分区留在屏幕外（用户看到的就是"展开了但找不到"）
        let wa = work_area_at(gx + nw / 2, gy + nh / 2);
        let (nx, ny) = clamp_into(wa, gx, gy, nw, nh);
        db.update_box_window_cfg(box_id, |c| {
            c.collapsed = false;
            c.expanded_geom = None;
            c.x = nx;
            c.y = ny;
            c.w = nw;
            c.h = nh;
        })?;
        let _ = win.set_position(PhysicalPosition::new(nx, ny));
        let _ = win.set_size(PhysicalSize::new(nw.max(1) as u32, nh.max(1) as u32));
        if !cfg.locked {
            let _ = win.set_resizable(true);
        }
        persist_cfg(&win, box_id, true);
    }
    // 通知前端按新状态重渲染（胶囊 <-> 完整分区）
    let _ = app.emit_to(
        &label,
        "box-capsule-changed",
        serde_json::json!({ "boxId": box_id, "collapsed": collapsed }),
    );
    Ok(())
}

/// 托盘开关：最小化 / 恢复全部分区与组件。
///
/// - **隐藏（最小化）**：把当前可见的分区、组件全部藏起来，并记录一份可见快照；
///   不写任何持久化配置（分区的 cfg.visible、组件的 enabled 设置都不动）。
/// - **再点一次（恢复）**：按快照精确还原——隐藏前可见的才恢复，
///   本来就没打开的分区 / 组件绝不会被顺手显示出来，也不会丢失。
pub fn toggle_all_boxes(app: &AppHandle) {
    use std::sync::{Mutex, OnceLock};
    static SNAP: OnceLock<Mutex<Option<(Vec<i64>, Vec<&'static str>)>>> = OnceLock::new();
    let store = SNAP.get_or_init(|| Mutex::new(None));
    let mut snap = store.lock().unwrap();
    let widgets: [&str; 6] = ["notes", "stocks", "huangli", "poetry", "huangdi", "shanghan"];

    // 已处于"最小化"状态 → 按快照恢复
    if let Some((boxes, ws)) = snap.take() {
        let bw = app.state::<BoxWindows>();
        let keep = keep_on_show_desktop(app);
        for id in boxes {
            if let Some(label) = bw.label_for(id) {
                if let Some(win) = app.get_webview_window(&label) {
                    let _ = win.show();
                    // hide/show 循环可能弄丢 Progman 父子关系，恢复时重新钉上
                    if keep {
                        apply_show_desktop_immune(&win, true);
                    }
                }
            }
        }
        for w in ws {
            if let Some(win) = app.get_webview_window(w) {
                let _ = win.show();
                if keep {
                    apply_show_desktop_immune(&win, true);
                }
            }
        }
        return;
    }

    // 显示状态 → 最小化：记录当前可见者并全部隐藏
    let bw = app.state::<BoxWindows>();
    let mut hidden_boxes: Vec<i64> = Vec::new();
    for id in bw.ids() {
        if let Some(label) = bw.label_for(id) {
            if let Some(win) = app.get_webview_window(&label) {
                if win.is_visible().unwrap_or(false) {
                    hidden_boxes.push(id);
                    hide_win(&win);
                }
            }
        }
    }
    let mut hidden_widgets: Vec<&'static str> = Vec::new();
    for w in widgets {
        if let Some(win) = app.get_webview_window(w) {
            if win.is_visible().unwrap_or(false) {
                hidden_widgets.push(w);
                hide_win(&win);
            }
        }
    }
    // 组件菜单是独立窗口，跟着一起收起，别留个孤儿浮在上面
    for kind in ["notes", "stock", "huangli", "poetry", "huangdi", "shanghan"] {
        if let Some(win) = app.get_webview_window(&format!("wmenu-{kind}")) {
            let _ = win.hide();
        }
    }
    *snap = Some((hidden_boxes, hidden_widgets));
}

/// 计算吸附目标：返回 (距离, 新x, 新y)
/// 在候选矩形里找拖动落点的吸附位置。
///
/// x、y 两轴**各自独立**挑最近的对齐边：以前是全场只挑一条最近的边，
/// x 差 4px、y 差 8px 时只把 x 对齐，y 仍歪着 —— 用户看起来就是「没吸附上」。
/// 返回 `None` 表示两轴都没有可对齐的边；命中时返回 (0, 吸附后 x, 吸附后 y)。
pub(crate) fn find_snap(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    others: &[(i64, i32, i32, i32, i32)],
    threshold: i32,
) -> Option<(i32, i32, i32)> {
    let mut bx: Option<(i32, i32)> = None; // (距离, 吸附后的 x)
    let mut by: Option<(i32, i32)> = None; // (距离, 吸附后的 y)
    for &(_, ox, oy, ow, oh) in others {
        let overlap_y = y < oy + oh && y + h > oy;
        let overlap_x = x < ox + ow && x + w > ox;
        if overlap_y {
            // 本窗口右边缘贴对方左边缘
            let d = (x + w - ox).abs();
            if d < threshold && bx.map_or(true, |(bd, _)| d < bd) {
                bx = Some((d, ox - w));
            }
            // 本窗口左边缘贴对方右边缘
            let d2 = (x - (ox + ow)).abs();
            if d2 < threshold && bx.map_or(true, |(bd, _)| d2 < bd) {
                bx = Some((d2, ox + ow));
            }
            // 同侧对齐：右边缘贴对方右边缘（拉伸/并排时最自然的对齐方式，
            // 也是贴屏幕工作区右边缘的唯一候选）
            let d3 = (x + w - (ox + ow)).abs();
            if d3 < threshold && bx.map_or(true, |(bd, _)| d3 < bd) {
                bx = Some((d3, ox + ow - w));
            }
            // 同侧对齐：左边缘贴对方左边缘
            let d4 = (x - ox).abs();
            if d4 < threshold && bx.map_or(true, |(bd, _)| d4 < bd) {
                bx = Some((d4, ox));
            }
        }
        if overlap_x {
            // 本窗口下边缘贴对方上边缘
            let d = (y + h - oy).abs();
            if d < threshold && by.map_or(true, |(bd, _)| d < bd) {
                by = Some((d, oy - h));
            }
            // 本窗口上边缘贴对方下边缘
            let d2 = (y - (oy + oh)).abs();
            if d2 < threshold && by.map_or(true, |(bd, _)| d2 < bd) {
                by = Some((d2, oy + oh));
            }
            // 同侧对齐：下边缘贴对方下边缘（含贴屏幕工作区/任务栏上沿）
            let d3 = (y + h - (oy + oh)).abs();
            if d3 < threshold && by.map_or(true, |(bd, _)| d3 < bd) {
                by = Some((d3, oy + oh - h));
            }
            // 同侧对齐：上边缘贴对方上边缘
            let d4 = (y - oy).abs();
            if d4 < threshold && by.map_or(true, |(bd, _)| d4 < bd) {
                by = Some((d4, oy));
            }
        }
    }
    match (bx, by) {
        (None, None) => None,
        (bx, by) => Some((
            0,
            bx.map(|(_, v)| v).unwrap_or(x),
            by.map(|(_, v)| v).unwrap_or(y),
        )),
    }
}

/// 拉伸吸附的候选矩形：其它分区 + 便签组件 + 所在屏幕的工作区，
/// 全部物理像素。前端拉宽/拉高时把移动中的边缘对齐到这些矩形的边上。
pub struct SnapTargets {
    /// [x, y, w, h]
    pub rects: Vec<[i32; 4]>,
    pub threshold: i32,
}

/// 设置里的「分区相互吸附」总开关（默认开）。关闭时前端拖动/拉伸吸附
/// 拿不到候选矩形，后端松手吸附也直接跳过 —— 所有吸附路径统一从这里判断。
pub(crate) fn snap_enabled(app: &AppHandle) -> bool {
    app.state::<Db>()
        .get_setting("app")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("snapEnabled").and_then(|b| b.as_bool()))
        .unwrap_or(true)
}

pub(crate) fn snap_targets(app: &AppHandle, owner: &str) -> SnapTargets {
    // 总开关关闭：候选为空 + 阈值 0，前端 magnetSnap/resizeSnap 自然全部不生效
    if !snap_enabled(app) {
        return SnapTargets {
            rects: Vec::new(),
            threshold: 0,
        };
    }
    let mut rects: Vec<[i32; 4]> = Vec::new();
    // 候选 = 其它可见分区 + 其它可见组件（便签/股票/黄历/诗词），排除自己
    let push_win = |label: &str, rects: &mut Vec<[i32; 4]>| {
        if label == owner {
            return;
        }
        if let Some(w) = app.get_webview_window(label) {
            if w.is_visible().unwrap_or(false) {
                if let (Ok(op), Ok(os)) = (w.outer_position(), w.outer_size()) {
                    rects.push([op.x, op.y, os.width as i32, os.height as i32]);
                }
            }
        }
    };
    if let Some(bw) = app.try_state::<BoxWindows>() {
        for id in bw.ids() {
            if let Some(l) = bw.label_for(id) {
                push_win(&l, &mut rects);
            }
        }
    }
    for l in [
        crate::notes::NOTE_LABEL,
        crate::stock::STOCK_LABEL,
        crate::huangli::HUANGLI_LABEL,
        crate::poetry::POETRY_LABEL,
    ] {
        push_win(l, &mut rects);
    }
    let mut threshold = 20;
    if let Some(win) = app.get_webview_window(owner) {
        threshold = app
            .state::<Db>()
            .get_setting("snapThreshold")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(20);
        // 所在屏幕的工作区也作为吸附目标（对齐屏幕边缘 / 任务栏）
        if let Ok(Some(mon)) = win.current_monitor() {
            let wa = mon.work_area();
            rects.push([
                wa.position.x,
                wa.position.y,
                wa.size.width as i32,
                wa.size.height as i32,
            ]);
        }
    }
    SnapTargets { rects, threshold }
}

/// 拖动/拉伸开始时都会取一次候选，顺带记日志：出问题时能立刻看出
/// 是"候选没取到"还是"吸附逻辑没命中"。
pub fn log_snap_targets(owner: &str, t: &SnapTargets) {
    crate::log_msg(&format!(
        "吸附候选: owner={}，{} 个矩形，阈值 {}",
        owner,
        t.rects.len(),
        t.threshold
    ));
}

/// 拖动吸附线程：拖动停止后自动对齐最近的分区（松手吸附，无抖动）
/// `force_moved`：前端确认窗口确实移动过（手动拖动路径），避免"点击头部也吸附"
pub fn start_box_drag(app: AppHandle, box_id: i64, force_moved: bool) -> Result<(), String> {
    let label = format!("box-{box_id}");
    let win = app
        .get_webview_window(&label)
        .ok_or_else(|| "分区窗口不存在".to_string())?;
    // 同一分区同时只允许一个吸附线程，防止叠加轮询导致卡顿
    let Some(bw) = app.try_state::<BoxWindows>() else {
        return Ok(());
    };
    if !bw.try_begin_drag(box_id) {
        return Ok(());
    }
    let bw2 = app.state::<BoxWindows>().inner().clone();
    std::thread::spawn(move || {
        struct Guard {
            bw: BoxWindows,
            id: i64,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                self.bw.end_drag(self.id);
            }
        }
        let _g = Guard { bw: bw2, id: box_id };
        let mut last_pos: Option<(i32, i32)> = None;
        let mut stable = 0u32;
        let mut ever_moved = force_moved;
        let begin = Instant::now();
        let mut ticks = 0u32;
        loop {
            ticks += 1;
            // 最多运行约 7 秒，防止任何情况下线程常驻
            if ticks > 240 {
                break;
            }
            let Ok(pos) = win.outer_position() else { break };
            let Ok(size) = win.outer_size() else { break };
            if let Some(lp) = last_pos {
                if lp == (pos.x, pos.y) {
                    stable += 1;
                } else {
                    ever_moved = true;
                    stable = 0;
                }
            }
            last_pos = Some((pos.x, pos.y));
            // 从未移动过且超过 1.5 秒 → 视为点击，不吸附
            if !ever_moved && begin.elapsed() > Duration::from_millis(1500) {
                break;
            }
            if stable >= 10 {
                if !ever_moved {
                    break;
                }
                // 设置里关了吸附 → 不做松手对齐，直接结束
                if !snap_enabled(&app) {
                    break;
                }
                // 拖动已停止，做一次对齐
                let mut others: Vec<(i64, i32, i32, i32, i32)> = Vec::new();
                if let Some(bw) = app.try_state::<BoxWindows>() {
                    for other_id in bw.ids() {
                        if other_id == box_id {
                            continue;
                        }
                        if let Some(l) = bw.label_for(other_id) {
                            if let Some(w) = app.get_webview_window(&l) {
                                if let (Ok(op), Ok(os)) = (w.outer_position(), w.outer_size()) {
                                    others.push((
                                        other_id,
                                        op.x,
                                        op.y,
                                        os.width as i32,
                                        os.height as i32,
                                    ));
                                }
                            }
                        }
                    }
                }
                // 便签组件窗口也参与吸附（id 用 -1 占位，不会与分区 id 冲突）
                if let Some(nw) = app.get_webview_window(crate::notes::NOTE_LABEL) {
                    if nw.is_visible().unwrap_or(false) {
                        if let (Ok(np), Ok(ns)) = (nw.outer_position(), nw.outer_size()) {
                            others.push((-1, np.x, np.y, ns.width as i32, ns.height as i32));
                        }
                    }
                }
                // 所在屏幕的工作区也参与吸附（id -2 占位）：贴屏幕边缘 / 任务栏上沿
                if let Ok(Some(mon)) = win.current_monitor() {
                    let wa = mon.work_area();
                    others.push((
                        -2,
                        wa.position.x,
                        wa.position.y,
                        wa.size.width as i32,
                        wa.size.height as i32,
                    ));
                }
                let threshold = app
                    .state::<Db>()
                    .get_setting("snapThreshold")
                    .ok()
                    .flatten()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(20);
                if let Some((_, nx, ny)) = find_snap(
                    pos.x,
                    pos.y,
                    size.width as i32,
                    size.height as i32,
                    &others,
                    threshold,
                ) {
                    crate::log_msg(&format!(
                        "拖动吸附: ({},{}) → ({},{})，候选 {} 个",
                        pos.x,
                        pos.y,
                        nx,
                        ny,
                        others.len()
                    ));
                    let _ = win.set_position(PhysicalPosition::new(nx, ny));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
    Ok(())
}

// ---------- 桌面右键菜单（Directory\Background） ----------

fn reg_run(args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let out = std::process::Command::new("reg")
        .args(args)
        // 不弹控制台黑窗
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "注册表操作失败: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

pub fn install_shell_menu() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_str = exe.to_string_lossy().to_string();
    let key = r"HKCU\Software\Classes\Directory\Background\shell\QidominAddBox";
    reg_run(&["add", key, "/ve", "/d", "新建 Qidomin 分区", "/f"])?;
    reg_run(&["add", key, "/v", "Icon", "/d", &exe_str, "/f"])?;
    reg_run(&[
        "add",
        &format!("{key}\\command"),
        "/ve",
        "/d",
        &format!("\"{exe_str}\" --add-box"),
        "/f",
    ])?;
    Ok(())
}

pub fn uninstall_shell_menu() -> Result<(), String> {
    let key = r"HKCU\Software\Classes\Directory\Background\shell\QidominAddBox";
    let _ = reg_run(&["delete", key, "/f"]);
    Ok(())
}

pub fn is_shell_menu_installed() -> bool {
    use std::os::windows::process::CommandExt;

    let key = r"HKCU\Software\Classes\Directory\Background\shell\QidominAddBox";
    std::process::Command::new("reg")
        .args(["query", key])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 重命名时同步窗口标题
pub fn rename_box_window(app: &AppHandle, box_id: i64, name: &str) {
    if let Some(label) = app
        .try_state::<BoxWindows>()
        .and_then(|b| b.label_for(box_id))
    {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.set_title(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{box_size_from_cfg, find_snap, DEFAULT_BOX_H, DEFAULT_BOX_W, MIN_BOX_H, MIN_BOX_W};
    use crate::db::BoxWindowCfg;

    /// 没有配置（新分区）→ 用默认尺寸，可见
    #[test]
    fn size_uses_default_when_cfg_missing() {
        let (w, h, visible, locked, degenerate) = box_size_from_cfg(None);
        assert_eq!((w, h), (DEFAULT_BOX_W, DEFAULT_BOX_H));
        assert!(visible);
        assert!(!locked);
        assert!(degenerate);
    }

    /// 回归：备份文件缺几何字段 → w/h 反序列化成 0 → 以前会建出 140×100
    /// 落在 (0,0) 的"碎片窗口"（锁定后彻底动不了）。现在必须退回默认尺寸，
    /// 同时保留用户配置里的 visible / locked 意图。
    #[test]
    fn size_falls_back_when_cfg_degenerate() {
        let mut cfg = BoxWindowCfg::new(7);
        cfg.w = 0;
        cfg.h = 0;
        cfg.x = 0;
        cfg.y = 0;
        cfg.visible = true;
        cfg.locked = true;
        let (w, h, visible, locked, degenerate) = box_size_from_cfg(Some(&cfg));
        assert_eq!((w, h), (DEFAULT_BOX_W, DEFAULT_BOX_H));
        assert!(visible);
        assert!(locked, "锁定是用户意图，不该因为几何坏掉被重置");
        assert!(degenerate);
    }

    /// 恰好等于下限的尺寸是合法的（不该被当成坏配置）
    #[test]
    fn size_accepts_exact_minimum() {
        let mut cfg = BoxWindowCfg::new(8);
        cfg.w = MIN_BOX_W;
        cfg.h = MIN_BOX_H;
        let (w, h, _, _, degenerate) = box_size_from_cfg(Some(&cfg));
        assert_eq!((w, h), (MIN_BOX_W, MIN_BOX_H));
        assert!(!degenerate);
    }

    /// 正常配置原样沿用
    #[test]
    fn size_keeps_valid_cfg() {
        let mut cfg = BoxWindowCfg::new(9);
        cfg.w = 472;
        cfg.h = 634;
        cfg.visible = false;
        let (w, h, visible, locked, degenerate) = box_size_from_cfg(Some(&cfg));
        assert_eq!((w, h), (472, 634));
        assert!(!visible);
        assert!(!locked);
        assert!(!degenerate);
    }

    /// 右边缘贴对方左边缘，同时上边缘贴对方上边缘（两轴独立）
    #[test]
    fn snap_aligns_right_edge_to_left_edge() {
        // 本窗口 (300,100,200,150)，对方 (480,80,200,120)
        // x：右边 500 vs 对方左边 480，差 20；y：上边 100 vs 对方上边 80，差 20
        let (_, nx, ny) = find_snap(300, 100, 200, 150, &[(1, 480, 80, 200, 120)], 25).unwrap();
        assert_eq!(nx, 280);
        assert_eq!(ny, 80);
    }

    /// 回归：x、y 两轴各自独立吸附。以前全场只挑一条最近的边，
    /// x 差 4px、y 差 8px 时只对 x，y 仍歪着 → 看起来"没吸附上"。
    #[test]
    fn snap_aligns_both_axes_independently() {
        // x 差 4（右边 504 vs 对方左边 500），y 差 8（下边 208 vs 对方上边 200）
        let (_, nx, ny) = find_snap(304, 108, 200, 100, &[(1, 500, 200, 300, 150)], 20).unwrap();
        assert_eq!(nx, 300); // x 吸附
        assert_eq!(ny, 100); // y 也吸附（以前保持 108 不动）
    }

    /// 超出阈值不吸附
    #[test]
    fn snap_ignores_beyond_threshold() {
        assert_eq!(
            find_snap(300, 100, 200, 150, &[(1, 600, 80, 200, 120)], 20),
            None
        );
    }

    /// 上边缘贴对方下边缘（垂直方向从上往下靠）
    #[test]
    fn snap_aligns_top_edge_to_bottom_edge() {
        let (_, _nx, ny) = find_snap(500, 340, 200, 100, &[(1, 480, 80, 300, 250)], 20).unwrap();
        assert_eq!(ny, 330);
    }

    /// 屏幕工作区（伪矩形）也能作为吸附目标
    #[test]
    fn snap_aligns_to_work_area_edges() {
        // 右边缘 1890 贴工作区右边 1920
        let (_, nx, _) = find_snap(1690, 100, 200, 150, &[(-2, 0, 0, 1920, 1040)], 40).unwrap();
        assert_eq!(nx, 1720);
    }
}
