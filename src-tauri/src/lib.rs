mod absorb;
mod boxwin;
mod clipboard;
mod commands;
mod db;
mod engine;
mod filedrag;
mod folder_watch;
mod huangdi;
mod huangli;
mod icons;
mod mousehotkey;
mod notes;
mod poetry;
mod resize_snap;
mod shellicon;
mod shellmenu;
mod shelldrop;
mod session;
mod stock;
mod search;
mod shanghan;
mod usn;

use boxwin::BoxWindows;
use db::Db;
/// 退出守卫进程的入口（见 [`session`]）：主进程被强杀时由它把文件还回桌面
pub use session::run_exit_guard;
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Listener, Manager,
};

/// 简易文件日志（用于卡死问题定位，写入 %APPDATA%\com.qidomin.app\qidomin.log）
pub fn log_msg(msg: &str) {
    use std::io::Write;
    let dir = std::env::var("APPDATA").unwrap_or_default();
    if dir.is_empty() {
        return;
    }
    let p = std::path::Path::new(&dir).join("com.qidomin.app").join("qidomin.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(
            f,
            "[{} pid={}] {}",
            chrono::Local::now().format("%H:%M:%S%.3f"),
            std::process::id(),
            msg
        );
    }
}

/// 枚举系统中其他 qidomin.exe 进程 pid（不含自身）
fn other_qidomin_pids() -> Vec<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut pids: Vec<u32> = Vec::new();
    unsafe {
        let self_pid = std::process::id();
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap_or_default();
        if snap.is_invalid() {
            return pids;
        }
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&pe.szExeFile)
                    .trim_end_matches('\0')
                    .to_lowercase();
                if name.ends_with("qidomin.exe") && pe.th32ProcessID != self_pid {
                    pids.push(pe.th32ProcessID);
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

/// 指定进程是否拥有任意顶层窗口
fn pid_has_window(target_pid: u32) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId};

    unsafe extern "system" fn probe(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = lparam.0 as *const (u32, AtomicBool);
        let (target_pid, found) = &*ctx;
        let mut win_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut win_pid));
        if win_pid == *target_pid {
            found.store(true, Ordering::SeqCst);
            return BOOL(0); // 找到即停
        }
        BOOL(1)
    }

    let found = AtomicBool::new(false);
    let ctx = (target_pid, &found);
    unsafe {
        let _ = EnumWindows(Some(probe), LPARAM(&ctx as *const _ as isize));
    }
    found.load(Ordering::SeqCst)
}

/// 是否存在"健康的其他 Qidomin 实例"：其拥有可响应（未卡死）的顶层窗口。
/// 主设置窗口是懒创建的，运行中不一定存在，所以按进程找窗口而不是按窗口标题找。
pub fn other_instance_alive() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_NULL,
    };

    let pids = other_qidomin_pids();
    if pids.is_empty() {
        return false;
    }

    unsafe extern "system" fn probe(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = lparam.0 as *const (Vec<u32>, AtomicBool);
        let (pids, alive) = &*ctx;
        let mut win_pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut win_pid));
        if pids.contains(&win_pid) {
            let mut result: usize = 0;
            // 窗口无响应（卡死）时返回 0，此时不算"健康实例"，由本实例接管
            let r = SendMessageTimeoutW(
                hwnd,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                800,
                Some(&mut result),
            );
            if r.0 != 0 {
                alive.store(true, Ordering::SeqCst);
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    let alive = AtomicBool::new(false);
    let ctx = (pids, &alive);
    unsafe {
        let _ = EnumWindows(Some(probe), LPARAM(&ctx as *const _ as isize));
    }
    alive.load(Ordering::SeqCst)
}

/// 清理"无可见窗口"的旧 qidomin 实例（卡死残留/孤儿），减少托盘重复图标。
/// 有窗口的实例不杀（可能是健康实例）。仅在单实例接管后调用。
pub fn kill_stale_instances() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };

    let pids = other_qidomin_pids();
    let mut killed = 0u32;
    unsafe {
        for pid in pids {
            if pid_has_window(pid) {
                continue; // 有窗口，可能是健康实例，不杀
            }
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                if TerminateProcess(h, 1).is_ok() {
                    killed += 1;
                }
                let _ = CloseHandle(h);
            }
        }
    }
    log_msg(&format!("kill_stale_instances: killed {killed}"));
}

/// 清理本应用残留的 WebView2 进程。
/// 卡死的旧实例不会销毁其 WebView，导致 msedgewebview2 浏览器进程长期残留并占用
/// 同一 user data folder；新实例（或新建窗口）与之握手时会永久挂起 → 卡死/无窗口。
/// 启动时在创建任何 WebView 前调用：只杀“父进程是 qidomin（任何实例）或父进程已不存在”的
/// msedgewebview2.exe，不触碰其他 WebView2 应用。
pub fn cleanup_stale_webview2() {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).unwrap_or_default();
        if snap.is_invalid() {
            return;
        }
        let mut procs: Vec<(u32, u32, Vec<u16>)> = Vec::new();
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                procs.push((pe.th32ProcessID, pe.th32ParentProcessID, pe.szExeFile.to_vec()));
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);

        let is_exe = |name: &[u16], target: &str| -> bool {
            let n = String::from_utf16_lossy(name)
                .trim_end_matches('\0')
                .to_lowercase();
            n == target || n.ends_with(&format!("\\{target}"))
        };

        let qidomin_pids: Vec<u32> = procs
            .iter()
            .filter(|(_, _, n)| is_exe(n, "qidomin.exe"))
            .map(|(p, _, _)| *p)
            .collect();
        let all_pids: std::collections::HashSet<u32> =
            procs.iter().map(|(p, _, _)| *p).collect();

        let mut killed = 0u32;
        for (pid, ppid, name) in &procs {
            if !is_exe(name, "msedgewebview2.exe") {
                continue;
            }
            // 父进程是 qidomin，或父进程已不存在（孤儿残留）→ 判定为本应用残留
            let orphan = !all_pids.contains(ppid);
            let parent_is_qidomin = qidomin_pids.contains(ppid);
            if !orphan && !parent_is_qidomin {
                continue;
            }
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, *pid) {
                if TerminateProcess(h, 1).is_ok() {
                    killed += 1;
                }
                let _ = CloseHandle(h);
            }
        }
        log_msg(&format!("cleanup_stale_webview2: killed {killed}"));
    }
}

/// 递归拷贝目录里的所有文件（含子目录），返回拷贝的文件数。
/// 只用于 identifier 变更后的一次性数据目录迁移。
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<usize> {
    let mut n = 0;
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            n += copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
            n += 1;
        }
    }
    Ok(n)
}

/// 读取应用内「界面缩放」（设置页可调，1.0 = 100%）。非法值回退 1.0。
///
/// 上限 `MAX_UI_ZOOM`：设置页只提供到 175%（2.0 那档已下掉）。这里一并夹住，
/// 否则老配置里存着 2.0 的用户会既看不到对应单选项（界面上像坏了），
/// 又继续被按 200% 渲染。
pub fn ui_zoom(app: &AppHandle) -> f64 {
    app.try_state::<Db>()
        .and_then(|db| db.get_setting("app").ok().flatten())
        .and_then(|s| serde_json::from_str::<crate::commands::AppSettings>(&s).ok())
        .map(|x| x.ui_zoom)
        .filter(|z| (0.5..=3.0).contains(z))
        // 老配置存过 2.0（200% 那档已下掉）→ 平滑收到上限，别硬夹成别的值
        .map(|z| z.min(MAX_UI_ZOOM))
        .unwrap_or(1.0)
}

/// 设置页可选的最大界面缩放（175%）。改这里要同步 SettingsView.tsx 里的档位数组。
pub const MAX_UI_ZOOM: f64 = 1.75;

/// 对窗口应用界面缩放：WebView 内容整体放大（等价于浏览器缩放）。
/// 固定尺寸窗口（设置 760x680 / 搜索 540x380）同步放大窗口本身，
/// 保证可视内容区域（CSS 像素）不变——视觉上和系统 DPI 缩放一致，只是字更大。
/// 分区 / 组件窗口尺寸由用户摆放，内容自适应重排，只缩放内容。
pub fn apply_ui_zoom(win: &tauri::WebviewWindow) {
    let z = ui_zoom(win.app_handle());
    if (z - 1.0).abs() < f64::EPSILON {
        return;
    }
    let _ = win.set_zoom(z);
    match win.label() {
        "main" => {
            let _ = win.set_size(tauri::LogicalSize::new(760.0 * z, 680.0 * z));
        }
        "search" => {
            let _ = win.set_size(tauri::LogicalSize::new(540.0 * z, 380.0 * z));
        }
        _ => {}
    }
}

/// 主设置窗口懒创建：启动阶段不创建（少一次 WebView2 初始化），首次需要时才建。
fn ensure_main_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(w) = app.get_webview_window("main") {
        return Ok(w);
    }
    let cfg = app
        .config()
        .app
        .windows
        .iter()
        .find(|c| c.label == "main")
        .cloned()
        .ok_or_else(|| "缺少 main 窗口配置".to_string())?;
    let win = tauri::WebviewWindowBuilder::from_config(app, &cfg)
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    // 设置窗口固定尺寸：不允许拖边缩放、不允许最大化（最大化按钮/快捷键也失效）
    let _ = win.set_resizable(false);
    let _ = win.set_maximizable(false);
    // 强制不在任务栏显示（Tauri 的 skip_taskbar 在无边框窗口上可能失效）
    boxwin::force_no_taskbar(&win);
    apply_ui_zoom(&win);
    // 关闭时隐藏而非销毁
    let w2 = win.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = w2.hide();
        }
    });
    Ok(win)
}

fn show_main(app: &AppHandle) {
    match ensure_main_window(app) {
        Ok(window) => {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        Err(e) => log_msg(&format!("ensure_main_window failed: {e}")),
    }
}

/// 显示 / 创建搜索框窗口（托盘左键单击触发）。
fn ensure_search_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(w) = app.get_webview_window("search") {
        return Ok(w);
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "search",
        tauri::WebviewUrl::App("index.html?search".into()),
    )
    .title("搜索")
    .inner_size(540.0, 380.0)
    .resizable(true)
    .decorations(false)
    .shadow(true)
    .skip_taskbar(true)
    .always_on_top(true)
    .visible(false)
    .build()
    .map_err(|e| format!("创建搜索窗口失败: {e}"))?;
    // 强制不在任务栏显示（Tauri 的 skip_taskbar 在透明无边框窗口上可能失效）
    boxwin::force_no_taskbar(&win);
    apply_ui_zoom(&win);
    let _ = win.set_always_on_top(true);
    // 前端首屏渲染完成信号：第一次显示要等这个信号，否则白屏一闪
    let _ = app.listen("search-window-ready", |_| {
        SEARCH_READY.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    // 拖动区双击会触发 maximize，搜索面板不允许最大化（会被撑爆）
    let _ = win.set_maximizable(false);
    // 关闭时隐藏而非销毁
    let w2 = win.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = w2.hide();
        }
    });
    Ok(win)
}

/// 解除 Windows 前台切换限制。
///
/// 后台进程直接调 SetForegroundWindow 会被系统拒绝——表现为「面板弹出来了，
/// 键盘焦点却还留在原程序」，打字全进了别的窗口。鼠标快捷键触发的路径尤其明显：
/// 中键被钩子吞掉，原程序从未失去前台。
/// 经典解法（PowerToys Run / Everything 同款）：先注入一次 Alt 按下+抬起，
/// 系统即认为有用户输入，随后的置前台请求放行。空按 Alt 无副作用。
fn unlock_foreground() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, INPUT_0, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };
    let mk = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0x12), // VK_MENU（Alt）
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        let inputs = [mk(false), mk(true)];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 搜索面板的弹出锚点（坐标一律用物理像素，和窗口定位口径一致）
pub(crate) enum SearchAnchor {
    /// 鼠标快捷键：面板**左下角对齐该点**，就地弹出
    Point(i32, i32),
    /// 托盘图标：面板**水平中心对齐图标中心**，垂直贴在图标上方
    /// （上方放不下就翻到图标下方）。cx/top/bottom 取托盘图标矩形。
    TrayIcon { cx: i32, top: i32, bottom: i32 },
}

/// 托盘左键单击 / 鼠标快捷键：切换搜索框（已显示则收起，否则弹出）。
pub(crate) fn show_search(app: &AppHandle, anchor: SearchAnchor) {
    // 入口日志：区分"托盘左键事件没到"和"搜索窗口自身问题"
    log_msg("搜索面板：show_search 触发");
    // 窗口实际尺寸会随界面缩放放大（见 apply_ui_zoom），定位常量同步跟随
    let zoom = ui_zoom(app);
    let search_w = 540.0 * zoom;
    let search_h = 380.0 * zoom;
    match ensure_search_window(app) {
        Ok(window) => {
            // 已显示则收起（再次触发切换）
            if window.is_visible().unwrap_or(false) {
                log_msg("搜索面板：已可见 → 收起（toggle）");
                let _ = window.hide();
                return;
            }
            match anchor {
                SearchAnchor::Point(cx, cy) => {
                    // 光标所在显示器（多屏支持），拿不到就退回主显示器
                    let mon = window
                        .monitor_from_point(cx as f64, cy as f64)
                        .ok()
                        .flatten()
                        .or_else(|| window.primary_monitor().ok().flatten());
                    if let Some(m) = mon {
                        let sf = m.scale_factor();
                        let mp = m.position();
                        let ms = m.size();
                        let w_phys = (search_w * sf).round() as i32;
                        let h_phys = (search_h * sf).round() as i32;
                        // 左下角对齐光标：左上角 = (cx, cy - 高度)
                        let mut x = cx;
                        let mut y = cy - h_phys;
                        // 贴边收进屏幕：光标贴近右/下边缘时别让面板跑出去
                        if x + w_phys > mp.x + ms.width as i32 {
                            x = mp.x + ms.width as i32 - w_phys;
                        }
                        if x < mp.x {
                            x = mp.x;
                        }
                        // 光标太靠上放不下时，翻转到光标下方弹出
                        if y < mp.y {
                            y = cy;
                        }
                        if y + h_phys > mp.y + ms.height as i32 {
                            y = mp.y + ms.height as i32 - h_phys;
                        }
                        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
                    }
                }
                SearchAnchor::TrayIcon { cx, top, bottom } => {
                    // 托盘图标所在的那块屏（多屏 / 任务栏在副屏也要对）
                    let mon = window
                        .monitor_from_point(cx as f64, top as f64)
                        .ok()
                        .flatten()
                        .or_else(|| window.primary_monitor().ok().flatten());
                    if let Some(m) = mon {
                        let sf = m.scale_factor();
                        let mp = m.position();
                        let ms = m.size();
                        // 宽度优先取窗口真实尺寸（用户可能拉过边框），拿不到才用常量
                        let (w_phys, h_phys) = window
                            .outer_size()
                            .map(|s| (s.width as i32, s.height as i32))
                            .unwrap_or((
                                (search_w * sf).round() as i32,
                                (search_h * sf).round() as i32,
                            ));
                        // 水平：面板中心 = 图标中心
                        let mut x = cx - w_phys / 2;
                        // 垂直：默认贴图标上沿（任务栏在屏幕下方时，就是"任务栏上方"）
                        let mut y = top - h_phys - 8;
                        // 左右贴边收进屏幕内
                        if x + w_phys > mp.x + ms.width as i32 {
                            x = mp.x + ms.width as i32 - w_phys;
                        }
                        if x < mp.x {
                            x = mp.x;
                        }
                        // 上方放不下（任务栏在屏幕上方 / 图标太靠上）→ 翻到图标下方
                        if y < mp.y {
                            y = bottom + 8;
                        }
                        if y + h_phys > mp.y + ms.height as i32 {
                            y = mp.y + ms.height as i32 - h_phys;
                        }
                        if y < mp.y {
                            y = mp.y;
                        }
                        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
                        log_msg(&format!(
                            "搜索面板：托盘锚点定位 ({x},{y})，尺寸 {w_phys}x{h_phys}"
                        ));
                    }
                }
            }
            // 首次创建：WebView 还在加载，立即显示会白屏。
            // 几何已定位好，等前端 ready 信号（2.5 秒超时兜底）再显示。
            if !SEARCH_READY.load(std::sync::atomic::Ordering::SeqCst) {
                let w = window.clone();
                std::thread::spawn(move || {
                    let begin = std::time::Instant::now();
                    while !SEARCH_READY.load(std::sync::atomic::Ordering::SeqCst)
                        && begin.elapsed() < std::time::Duration::from_millis(2500)
                    {
                        std::thread::sleep(std::time::Duration::from_millis(40));
                    }
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_always_on_top(true);
                    unlock_foreground();
                    let _ = w.set_focus();
                });
                return;
            }
            let _ = window.show();
            let _ = window.unminimize();
            // 每次弹出重申置顶，保证面板永远在最上层
            let _ = window.set_always_on_top(true);
            // 解除前台锁后再置焦，否则中键弹出时键盘焦点还在原程序，打不了字
            unlock_foreground();
            let _ = window.set_focus();
        }
        Err(e) => log_msg(&format!("ensure_search_window failed: {e}")),
    }
}

/// 新建一个默认命名分区，并打开其分区窗口。返回最终用的分区名。
pub(crate) fn create_box_and_open(
    app: &AppHandle,
    db: &Db,
    name: &str,
    color: &str,
) -> Result<String, String> {
    // 名字统一交给后端定：`name` 只是"想要的"，被占用会自动换成可用的
    let name = commands::resolve_box_name(db, name, "file")?;
    let box_id = commands::insert_box_row(db, &name, color, "file", None)?;
    if let Err(e) = engine::ensure_box_dir(&name) {
        let _ = db
            .conn
            .lock()
            .map(|c| c.execute("DELETE FROM boxes WHERE id = ?1", rusqlite::params![box_id]));
        return Err(e);
    }
    boxwin::open_box_window(app, db, box_id, &name, color)?;
    if let Err(e) = boxwin::set_box_visible(app, box_id, true) {
        log_msg(&format!("create_box {name} 显示窗口失败：{e}"));
    }
    Ok(name)
}

/// 托盘 / 鼠标快捷键 / 桌面右键「新建分区」用的默认名。
///
/// 从**已有"新分区 N"的最大序号**往下取，而不是按分区总数取：
/// 总数会随着解散分区变小，取出来的号就会撞上还在用的名字，
/// 于是"新建分区"点了没反应（见 `commands::resolve_box_name` 的说明）。
pub(crate) fn next_box_name(db: &Db) -> String {
    engine::pick_free_box_name(db, "新分区", true).unwrap_or_else(|_| "新分区".to_string())
}

/// 处理 `--add-box` 启动参数（桌面右键菜单触发）
fn handle_add_box_args(app: &AppHandle, db: &Db, args: &[String]) {
    if args.iter().any(|a| a == "--add-box") {
        let name = next_box_name(db);
        let _ = create_box_and_open(app, db, &name, "#15b6c9");
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // 防御：清理可能存在的历史托盘，确保任务栏始终只有一个图标。
    // "main" 是 tauri.conf.json 里 trayIcon 配置自动创建的默认 id（现已移除，
    // 这里保留清理以防旧配置/旧实例残留），"main-tray" 是本函数上一次创建的同名托盘。
    let _ = app.remove_tray_by_id("main");
    let _ = app.remove_tray_by_id("main-tray");
    // 菜单不再用 Tauri 的原生 HMENU（Windows 上样式老旧、无法定制），
    // 改为右键弹出自绘的 Win11 风格菜单窗口（见 show_tray_menu / TrayMenuWindow.tsx）。
    TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().expect("missing app icon").clone())
        .tooltip("Qidomin")
        .on_tray_icon_event(|tray, event| {
            match event {
                // 左键：弹出快速搜索面板（保持原有行为）
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    rect,
                    position,
                    ..
                } => {
                    let handle = tray.app_handle();
                    // tauri 的 Rect 是"物理/逻辑"枚举；Windows 上托盘给的是物理像素，
                    // 真出现逻辑值就按主屏缩放换算（缩放只影响这一种情况）。
                    let sf = handle
                        .primary_monitor()
                        .ok()
                        .flatten()
                        .map(|m| m.scale_factor())
                        .unwrap_or(1.0);
                    let pos = rect.position.to_physical::<i32>(sf);
                    let size = rect.size.to_physical::<u32>(sf);
                    // 面板水平中心对到图标中心；上下沿用来决定贴在图标上方还是下方。
                    // 万一拿不到图标矩形（宽为 0），退回光标位置，至少不会甩到屏幕另一头。
                    let (cx, top, bottom) = if size.width > 0 {
                        (
                            pos.x + size.width as i32 / 2,
                            pos.y,
                            pos.y + size.height as i32,
                        )
                    } else {
                        let x = position.x.round() as i32;
                        let y = position.y.round() as i32;
                        (x, y, y)
                    };
                    // 托盘回调运行在主线程：回调里绝不能做重活（建 WebView 窗口要
                    // 阻塞主线程约 2s，Windows 会把应用判为"未响应"，ghost 化后
                    // 吞掉后续所有托盘点击——实测点过一次"设置"后托盘彻底失灵）。
                    // 这里只打包参数，重活全部丢给后台线程。
                    let app = handle.clone();
                    std::thread::spawn(move || {
                        show_search(&app, SearchAnchor::TrayIcon { cx, top, bottom })
                    });
                }
                // 右键：弹出 Win11 风格菜单窗口
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    rect,
                    position,
                    ..
                } => {
                    let handle = tray.app_handle();
                    let sf = handle
                        .primary_monitor()
                        .ok()
                        .flatten()
                        .map(|m| m.scale_factor())
                        .unwrap_or(1.0);
                    let pos = rect.position.to_physical::<i32>(sf);
                    let size = rect.size.to_physical::<u32>(sf);
                    let (cx, top, bottom) = if size.width > 0 {
                        (
                            pos.x + size.width as i32 / 2,
                            pos.y,
                            pos.y + size.height as i32,
                        )
                    } else {
                        let x = position.x.round() as i32;
                        let y = position.y.round() as i32;
                        (x, y, y)
                    };
                    // 同左键：回调只打包参数，建窗口的重活在后台线程做
                    let app = handle.clone();
                    std::thread::spawn(move || show_tray_menu(&app, cx, top, bottom));
                }
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}

/// 托盘菜单窗口的尺寸（逻辑像素）。项数固定：6 项 + 2 条分隔线。
/// 与 TrayMenuWindow.tsx 的布局保持一致，改了菜单项记得同步这里。
const TRAY_MENU_W: f64 = 252.0;
const TRAY_MENU_H: f64 = 264.0;

/// 主线程最后一次"空闲"的时刻（后台线程写入，主线程读到即证明它转得动）。
static MAIN_THREAD_ALIVE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// 毫秒时间戳 → 自增计数（只需比较大小，用 AtomicU64 存 unix 毫秒即可）。
fn now_ms_u64() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 托盘图标自愈：Windows 会把"主线程长时间不泵消息"的应用判为未响应，
/// 并**停止向它的托盘图标投递点击**（ghost 化）。主线程恢复后图标已经是死的，
/// 托盘右键永远不再触发 `on_tray_icon_event` —— 表现为"点一次新建分区/设置，
/// 菜单就再也弹不出来"。日志里主线程心跳照常继续，正是这个特征。
///
/// 判定用"心跳戳距今多久"，而不是"戳变没变"：心跳探针本身就是 10s 一跳，
/// 看变化会在每个周期里误判成卡死。这里要求**超过 25s 没更新**（两个多周期）
/// 才认定主线程真的卡住了。
fn spawn_tray_icon_watchdog(app: AppHandle) {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    std::thread::spawn(move || {
        // 宽限期：启动阶段本来就有几次秒级的主线程占用（分区窗口逐个创建），
        // 那时 Windows 还没判定未响应，不该动托盘。
        let start = Instant::now();
        // 上次"派活出去"的时间：修复任务在独立线程里跑（可能要等 60s），
        // 这里只记派活时刻做节流，避免它在等的这段时间里被反复派出去。
        let mut last_dispatch: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(1000));
            if start.elapsed() < Duration::from_secs(20) {
                continue;
            }
            let last = MAIN_THREAD_ALIVE.load(Ordering::Relaxed);
            if last == 0 {
                continue; // 还没收到过心跳，探针本身没起来
            }
            let age_ms = now_ms_u64().saturating_sub(last);
            if age_ms <= 25_000 {
                continue; // 主线程正常推进
            }
            // 节流：一次卡死只派一次活；派活后 90s 内不再重复
            if let Some(t) = last_dispatch {
                if t.elapsed() < Duration::from_secs(90) {
                    continue;
                }
            }
            last_dispatch = Some(Instant::now());
            log_msg(&format!(
                "tray-icon: 主线程已 {age_ms}ms 无心跳，等待恢复后重建托盘图标"
            ));
            let stuck_at = last;
            let app = app.clone();
            std::thread::spawn(move || {
                // 等主线程自己缓过来（最多 60s）
                let deadline = Instant::now() + Duration::from_secs(60);
                while Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(200));
                    if MAIN_THREAD_ALIVE.load(Ordering::Relaxed) != stuck_at {
                        break;
                    }
                }
                // 把重建投递回主线程：托盘图标的增删必须在主线程做
                let a = app.clone();
                let _ = app.run_on_main_thread(move || match build_tray(&a) {
                    Ok(()) => log_msg("tray-icon: 主线程曾阻塞，已重建托盘图标解除 ghost"),
                    Err(e) => log_msg(&format!("tray-icon: 重建托盘图标失败：{e}")),
                });
            });
        }
    });
}

/// 主线程心跳探针：后台线程每 10s 往主线程派发一次日志。
/// 主线程活着 → 日志每 10s 一条 "main-thread 心跳"；
/// 主线程死锁/被重活阻塞 → 心跳断流（delayed dispatch 也排队）。
/// 用来区分"进程活着但主线程死了"（托盘无响应但日志还有其它线程活动）。
fn spawn_main_thread_probe(app: AppHandle) {
    std::thread::spawn(move || {
        let mut n = 0u64;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));
            let app = app.clone();
            let seq = n;
            let _ = app.run_on_main_thread(move || {
                // 顺带刷新"主线程存活"戳：托盘图标自愈靠它判断主线程是否脱困
                MAIN_THREAD_ALIVE.store(now_ms_u64(), std::sync::atomic::Ordering::Relaxed);
                log_msg(&format!("main-thread 心跳 #{seq}（主线程存活）"));
            });
            n += 1;
        }
    });
}

/// 托盘菜单窗口的 label —— **常驻唯一窗口**。
///
/// 生命周期方案（经过三轮实测教训得出的最终形态）：
/// - 启动时预创建，WebView 常驻加载完成（弹出零等待、内容即时）
/// - 弹出 = 挪到托盘旁 + show；收起 = **挪到屏幕外**，永不 hide/destroy
/// - 为什么不用 hide/show 复用：WebView2 隐藏后渲染/脚本会挂起，show 后
///   存在不恢复的竞态（实测菜单弹出几次后前端心跳消失）
/// - 为什么不每次重建：动态建第二个 WebView 会和主窗口的 WebView 初始化
///   竞争（实测新窗口永远不加载内容、主线程心跳断流）
/// - 屏幕外方案下 WebView 始终可见渲染，两类竞态都不存在
const TRAY_MENU_LABEL: &str = "tray-menu";
/// 屏幕外隐藏位（物理像素）。-32000 是 Windows 传统"屏幕外"坐标，
/// 任何多屏组合都够不到。
fn tray_menu_offscreen() -> tauri::PhysicalPosition<i32> {
    tauri::PhysicalPosition::new(-32000, -32000)
}

/// 菜单弹出代际：每次弹出 +1，失焦轮询发现代际变了就自己退役
/// （保证任意时刻只有一个活跃轮询，旧的不会把新弹出的菜单收掉）。
static TRAY_MENU_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 菜单当前是否处于"展开"状态（屏幕外 = false）。收起路径统一走这里。
static TRAY_MENU_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 上一次 show_tray_menu 的时刻。Windows 托盘对一次右键可能连发两次 Click
/// （日志实测间隔 13ms），不去重的话会双 spawn 轮询、双 emit。
static LAST_TRAY_MENU_SHOW: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// 托盘菜单 WebView 最近一次心跳（前端 ping / shown 回执都会刷新）。
/// WebView 脚本会被静默冻死（窗口保留最后一帧渲染，看起来正常但点击、
/// 事件全无响应）——弹出时检查心跳龄，断流就销毁重建实现自愈。
static LAST_TRAY_MENU_HEARTBEAT: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);

/// 记录托盘菜单 WebView 心跳（tray_menu_ping 命令与 shown 事件回执共用）。
pub fn touch_tray_menu_heartbeat() {
    *LAST_TRAY_MENU_HEARTBEAT.lock().unwrap() = Some(std::time::Instant::now());
}

/// 在托盘附近弹出 Win11 风格的右键菜单窗口。
///
/// `cx`/`top`/`bottom` 是托盘图标的物理坐标（水平中心 / 上沿 / 下沿）：
/// 默认贴在图标下方；下方放不下就翻到上方。
fn show_tray_menu(app: &AppHandle, cx: i32, top: i32, bottom: i32) {
    // 一次右键连发两次 Click（间隔 ~13ms，日志实证）：300ms 内的重复调用直接吞掉
    {
        let mut last = LAST_TRAY_MENU_SHOW.lock().unwrap();
        if let Some(t) = *last {
            if t.elapsed() < std::time::Duration::from_millis(300) {
                log_msg("tray-menu：300ms 内重复触发，已去重");
                return;
            }
        }
        *last = Some(std::time::Instant::now());
    }
    let sf = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);
    let w = (TRAY_MENU_W * sf).round() as i32;
    let h = (TRAY_MENU_H * sf).round() as i32;
    // 水平：菜单中心对齐图标中心，两端夹回屏幕内（托盘图标可能贴着屏幕边）
    let (screen_w, screen_h) = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| (m.size().width as i32, m.size().height as i32))
        .unwrap_or((1920, 1080));
    let mut x = cx - w / 2;
    x = x.clamp(8, (screen_w - w - 8).max(8));
    // 垂直：默认在图标下方留 6px 缝；放不下（任务栏在底部时菜单会被截）就翻上去。
    // 注意跟**屏幕高度**比 —— 之前误写成跟宽度比，高分辨率屏上永远"放得下"，
    // 菜单就被定位到屏幕底边之外（日志里 y=1446 > 屏高 1440，整窗看不见）。
    let mut y = bottom + 6;
    if y + h > screen_h {
        y = (top - 6 - h).max(8);
    }
    // 窗口操作的 Result 统一检查：这些调用失败不会 panic，但会直接导致
    // "菜单没弹出来"—— 必须留痕，否则链路在这一环断掉时无迹可寻。
    let check = |what: &str, r: tauri::Result<()>| {
        if let Err(e) = r {
            log_msg(&format!("tray-menu {what} 失败：{e}"));
        }
    };
    // 常驻窗口复用；弹出前先做"WebView 活性"体检：
    // 心跳断流（>25s 无 ping/无 shown 回执）说明脚本已冻死 —— 窗口渲染保留
    // 最后一帧、看起来正常但点击/事件全死。销毁重建是唯一恢复手段。
    let win = match app.get_webview_window(TRAY_MENU_LABEL) {
        Some(w) => {
            let stale = match LAST_TRAY_MENU_HEARTBEAT.lock().unwrap().as_ref() {
                Some(t) => t.elapsed() > std::time::Duration::from_secs(25),
                // 从未收到过心跳：预创建的页面加载本身就需要一点时间，
                // 给 60s 宽限；连这都没有说明 WebView 从未活过
                None => false,
            };
            if stale {
                log_msg("tray-menu WebView 心跳断流，销毁重建自愈");
                let _ = w.destroy();
                precreate_tray_menu(app);
                // 重建后的页面在 ~150ms 内加载完（历史实测），照常继续弹出
                match app.get_webview_window(TRAY_MENU_LABEL) {
                    Some(w2) => w2,
                    None => {
                        log_msg("tray-menu 自愈重建失败，本次不弹菜单");
                        return;
                    }
                }
            } else {
                w
            }
        }
        None => {
            log_msg("tray-menu 常驻窗口不存在，尝试补建");
            precreate_tray_menu(app);
            match app.get_webview_window(TRAY_MENU_LABEL) {
                Some(w) => w,
                None => {
                    log_msg("tray-menu 补建失败，本次不弹菜单");
                    return;
                }
            }
        }
    };
    check("set_position", win.set_position(tauri::PhysicalPosition::new(x, y)));
    check("show", win.show());
    check("set_focus", win.set_focus());
    TRAY_MENU_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
    let gen = TRAY_MENU_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    log_msg(&format!("tray-menu 已弹出（gen={gen}），定位 ({x},{y})"));
    // 通知前端刷新（开机自启勾选态可能已变；这条事件也是前端活着的探针）
    if let Err(e) = app.emit_to(TRAY_MENU_LABEL, "tray-menu-shown", gen) {
        log_msg(&format!("tray-menu emit shown 失败：{e}"));
    }
    // 失焦收起 watcher：只喂"窗口矩形缓存 + 托盘点 + 代际"，轮询内零窗口 API
    spawn_tray_menu_blur_watcher(win, (x, y, w, h), (cx, top, bottom), gen);
}

/// 创建（或确认存在）托盘菜单常驻窗口。
/// 启动时调用：建好放屏幕外，WebView 立即开始加载页面，右键弹出零等待。
/// 弹出时若窗口意外不存在也会调用它兜底，所以入口处先查重。
fn precreate_tray_menu(app: &AppHandle) {
    if app.get_webview_window(TRAY_MENU_LABEL).is_some() {
        return;
    }
    let builder = tauri::WebviewWindowBuilder::new(
        app,
        TRAY_MENU_LABEL,
        tauri::WebviewUrl::App("index.html?traymenu=1".into()),
    )
    .title("Qidomin")
    .inner_size(TRAY_MENU_W, TRAY_MENU_H)
    .position(-32000.0, -32000.0)
    .decorations(false)
    .transparent(true)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .shadow(false)
    .visible(false);
    match builder.build() {
        Ok(win) => {
            let _ = win.set_position(tray_menu_offscreen());
            log_msg("tray-menu 窗口已预创建（屏幕外待命）");
        }
        Err(e) => log_msg(&format!("tray-menu 预创建失败：{e}")),
    }
}

/// 收起托盘菜单（挪到屏幕外，窗口与 WebView 常驻待命）。
/// 三个入口共用：菜单项点击后前端调用、Esc、失焦轮询。
pub fn tray_menu_dismiss(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(TRAY_MENU_LABEL) {
        let _ = win.set_position(tray_menu_offscreen());
    }
    TRAY_MENU_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
}

/// 托盘菜单的失焦自动收起（后台轮询）。
///
/// 只信鼠标、不信焦点：托盘右键后 Windows 的前台焦点在任务栏和菜单之间
/// 拉扯，is_focused 抖动不可信（实测会把刚弹出的菜单 1 秒内误收）。
/// 鼠标规则（菜单窗口矩形 + 托盘图标附近）：
/// - 鼠标悬停在菜单上 → 永不收
/// - 鼠标停在托盘附近（刚右键还没挪）→ 不收
/// - 从未进入过菜单、鼠标又不在托盘附近超过 3s → 收（弹出后没碰它）
/// - 曾进入菜单、离开超过 0.6s 且不在托盘附近 → 收（看过菜单后点去别处）
/// 代际号保证每次弹出只有一个活跃轮询。
fn spawn_tray_menu_blur_watcher(
    win: tauri::WebviewWindow,
    rect: (i32, i32, i32, i32),
    tray: (i32, i32, i32),
    gen: u64,
) {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    // 用普通线程而不是 tokio：项目未直接依赖 tokio，且轮询频率低、
    // 线程存活期短，开销可以忽略。
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut ever_hover = false;
        let mut left_hover_at: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            // 已收起（菜单项点击 / Esc / 失焦）或已有新代际弹出 → 本轮询退役
            if !TRAY_MENU_OPEN.load(Ordering::Relaxed)
                || TRAY_MENU_GEN.load(Ordering::Relaxed) != gen
            {
                break;
            }
            // 鼠标位置分类：菜单上 / 托盘附近 / 其它。
            // 窗口矩形用 show_tray_menu 缓存值，这里只调 GetCursorPos，
            // 不做任何跨线程窗口 API（避免与主线程互相拖累）。
            let Some((mx, my)) = cursor_point() else { break };
            let on_menu = mx >= rect.0
                && mx < rect.0 + rect.2
                && my >= rect.1
                && my < rect.1 + rect.3;
            let near_tray = (mx - tray.0).abs() < 90 && (my - (tray.1 + tray.2) / 2).abs() < 70;
            if on_menu {
                ever_hover = true;
                left_hover_at = None;
            } else if ever_hover && left_hover_at.is_none() {
                left_hover_at = Some(Instant::now());
            }
            // 收起判定（只看鼠标意图，焦点完全不参与）：
            let should_close = if on_menu || near_tray {
                false
            } else if ever_hover {
                // 看过菜单：离开 0.6s 才收（给"点菜单项"的鼠标轨迹留裕量）
                left_hover_at.is_some_and(|t| t.elapsed() > Duration::from_millis(600))
            } else {
                // 从没碰过菜单：3s 后收（弹出后放着不管）
                started.elapsed() > Duration::from_secs(3)
            };
            if should_close {
                // 收起 = 挪屏幕外（窗口常驻，不 hide——hide 会让 WebView 挂起）
                tray_menu_dismiss(win.app_handle());
                log_msg("tray-menu 失焦自动收起");
                break;
            }
        }
    });
}

/// 当前鼠标位置（物理像素）。拿不到（极少见）返回 None。
#[cfg(windows)]
fn cursor_point() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT::default();
    let ok = unsafe { GetCursorPos(&mut pt) };
    ok.ok().map(|_| (pt.x, pt.y))
}

#[cfg(not(windows))]
fn cursor_point() -> Option<(i32, i32)> {
    None
}

/// 退出前的收尾（托盘菜单"退出"调用）：归还文件 → 落盘 → 退出。
pub fn quit_app(app: &AppHandle) {
    // 先销"正常退出"标记，再做落盘/归还（这一步可能被打断或失败，
    // 但只要标记没了，下次启动就会按搬运记录把文件收回分区）
    session::mark_clean_exit();
    // 退出前把分区窗口的最终几何强制落盘（节流可能丢最后一帧），
    // 保证重启后的布局和退出那一刻一致
    boxwin::persist_all_geometry(app);
    // 退出前把分区里的文件归还桌面：程序没开的时候桌面是"正常"的，
    // 下次启动再按移动记录收回来（engine::reorganize_from_history）
    let db = app.state::<Db>();
    let n = match engine::return_all_to_desktop(&db) {
        Ok(n) => {
            log_msg(&format!("退出归还文件：{n} 个"));
            n
        }
        Err(e) => {
            log_msg(&format!("return_all_to_desktop failed: {e}"));
            0
        }
    };
    if n > 0 {
        shellicon::refresh_desktop_icons();
    }
    // 标记在进这条分支时就已经销掉了（见上）：守卫进程看到标记没了就不会再动文件
    app.exit(0);
}

/// 单实例检测：已有健康实例则返回 true（本实例应退出）；已有实例卡死/无响应则返回 false（本实例接管继续启动）
pub fn check_single_instance() -> bool {
    // 主判断：独占文件锁（内核级可靠，进程退出/崩溃自动释放，第二实例立即退出）
    if let Some(dir) = std::env::var_os("APPDATA") {
        let p = std::path::Path::new(&dir)
            .join("com.qidomin.app")
            .join("instance.lock");
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(f) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&p)
        {
            match f.try_lock() {
                Ok(_) => {
                    // 本实例持有锁：泄漏句柄使其保持到进程退出（系统自动释放）
                    let _ = std::mem::ManuallyDrop::new(f);
                    return false;
                }
                Err(_) => return true, // 已有实例在运行 → 本实例退出
            }
        }
    }
    // 兜底：进程 + 窗口探测（锁文件不可用时）
    // 已有其他 qidomin 实例且其窗口可响应 → 健康实例存在，本实例退出；
    // 若窗口无响应（卡死），则本实例接管继续启动。
    if other_instance_alive() {
        return true;
    }
    // 写实例标记文件（供排查，不参与判断）
    if let Some(dir) = std::env::var_os("APPDATA") {
        let p = std::path::Path::new(&dir)
            .join("com.qidomin.app")
            .join("instance.lock");
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(&p, std::process::id().to_string());
    }
    false
}

/// 搜索窗口前端是否已渲染完成（首次显示前等待它，避免白屏）
static SEARCH_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // 图标 / 缩略图走自定义协议：后端只给前端一个短 URL，图片由 WebView 自己按需
        // 拉取（前端再配合 loading="lazy"，只有看得见的那几张真的会触发提取）。
        // 相比"把 base64 塞进 IPC 返回值"，大文件夹的刷新从"搬几 MB 字符串 + JSON.parse"
        // 变成"搬几个短字符串" —— 这是分区列表卡不卡的关键。
        .register_asynchronous_uri_scheme_protocol(icons::PROTOCOL, |_ctx, request, responder| {
            let uri = request.uri().to_string();
            // 排队封顶：大目录快速滚动时不再无限创建线程（每条都要排队等提取许可）
            if !icons::try_begin_request() {
                // 503 + no-store：让 WebView 稍后重试，而不是把空白永久缓存下来
                let resp = tauri::http::Response::builder()
                    .status(503)
                    .header("Cache-Control", "no-store")
                    .body(Vec::new())
                    .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()));
                responder.respond(resp);
                return;
            }
            // 提取要走 Shell 缩略图处理器，可能几十毫秒，别占着 WebView 的请求线程
            std::thread::spawn(move || {
                let body = match icons::parse_icon_url(&uri) {
                    Some((size, path)) => {
                        // 限并发：一次放几十条线程进去只会互相抢 IO/CPU
                        let _permit = icons::acquire_extract_permit();
                        icons::icon_png(&path, size)
                    }
                    None => Vec::new(),
                };
                icons::end_request();
                let resp = tauri::http::Response::builder()
                    .header("Content-Type", "image/png")
                    // URL 里带了缓存代数，清缓存后地址会变，所以这里可以放心长缓存
                    .header("Cache-Control", "max-age=604800")
                    .body(body)
                    .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()));
                responder.respond(resp);
            });
        })
        .manage(BoxWindows::new())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            // 一次性迁移：identifier 已从 com.qidomin.app 改为 com.qidomin.desktop，
            // 数据目录跟着变了。老目录里有数据库而新目录还没有 → 整目录搬过来，
            // 老用户的分区配置 / 便签 / 设置不会"丢"。
            if let Some(parent) = data_dir.parent() {
                let old_dir = parent.join("com.qidomin.app");
                if old_dir != data_dir
                    && old_dir.join("qidomin.db").is_file()
                    && !data_dir.join("qidomin.db").exists()
                {
                    match copy_dir_recursive(&old_dir, &data_dir) {
                        Ok(n) => log_msg(&format!("已从旧数据目录迁移 {} 个文件", n)),
                        Err(e) => log_msg(&format!("旧数据目录迁移失败: {}", e)),
                    }
                }
            }
            // 记下主线程：分区窗口的拖放落点必须注册在这个线程上（见 shelldrop）
            shelldrop::mark_main_thread();
            // 上次是不是"非正常退出"（崩溃 / 被强杀 / 断电）？
            // 必须在 mark_start 之前认领，否则会把**自己**刚写的标记认走。
            // 非正常退出时，启动这条链路改成"把文件还回桌面"，而不是照常收回分区。
            let unclean_exit = session::claim_stale_session();
            if unclean_exit {
                log_msg("上次不是正常退出（崩溃/被强杀），本次启动先把文件还回桌面");
            }
            session::mark_start();
            let database = Db::open(&data_dir.join("qidomin.db"))?;
            app.manage(database);
            // 关联文件夹实时监控：给每个 link_path 挂目录监视器，文件增删即时刷新分区
            app.manage(folder_watch::FolderWatcher::new(app.handle().clone()));
            {
                let db = app.state::<Db>().inner().clone();
                app.state::<folder_watch::FolderWatcher>()
                    .start_watching_all(&db);
            }
            // 开机自启自愈：老版本写进 Run 的路径可能已失效（指向被删的旧 exe）
            // 或没加引号，启动时纠正一次。放后台线程，别拖慢首帧。
            let db_for_auto_start = app.state::<Db>().inner().clone();
            std::thread::spawn(move || {
                let _ = engine::repair_auto_start(&db_for_auto_start);
            });
            build_tray(app.handle())?;
            // 托盘菜单前端心跳（事件通道）：与 invoke 通道互为对照。
            // emit 通而 invoke 不通 = capability/命令注册问题；两条都不通 = WebView 没跑。
            // 在这里注册而不是放进 build_tray —— watchdog 修复托盘时会重复调用
            // build_tray，监听器注册多份会让同一条心跳日志打好几遍。
            app.listen("tray-menu-alive", |ev| {
                log_msg(&format!("tray-menu 前端心跳(事件)：{}", ev.payload()));
            });
            spawn_main_thread_probe(app.handle().clone());
            // 托盘图标自愈：主线程被重活堵到被 Windows 判为未响应时，
            // 托盘图标会被 ghost 化（不再投递点击）且不会自行恢复，这里负责救回来。
            spawn_tray_icon_watchdog(app.handle().clone());
            // 托盘菜单窗口常驻预创建：启动即加载页面，右键弹出零等待
            precreate_tray_menu(app.handle());
            // 搜索索引：后台线程先建一次索引，之后定时增量重建常用目录
            search::spawn_indexer(app.handle().clone());
            // 实时增量：基于 Windows 变更通知（USN Journal）监听已索引根的增删改名
            usn::start_watchers(app.handle().clone());
            // 主设置窗口配置为 create=false，启动时不创建，由 show_main 懒创建，
            // 这样启动阶段不需要初始化它的 WebView2（关闭行为/任务栏隐藏见 ensure_main_window）。
            // 桌面右键菜单已改为分区标题栏按钮入口，启动时静默移除旧注册。
            // reg.exe 是独立进程，放后台线程执行，避免阻塞启动。
            std::thread::spawn(|| {
                let _ = boxwin::uninstall_shell_menu();
            });

            // 便签到点提醒：后台线程轮询（15 秒一次）
            notes::spawn_reminder_loop(app.handle().clone());

            // 鼠标快捷键：全局低级钩子（总开关关闭时一律放行，零影响）
            mousehotkey::install(app.handle().clone());
            // 恢复上次保存的鼠标快捷键配置：钩子的生效状态存在模块静态表里，
            // 不在这里加载的话，刚启动时是「关闭+空表」，要进设置开关一次才生效
            {
                let db = app.state::<Db>().inner().clone();
                if let Ok(Some(json)) = db.get_setting("app") {
                    if let Ok(s) = serde_json::from_str::<crate::commands::AppSettings>(&json) {
                        mousehotkey::update_config(s.mouse_hotkeys_enabled, s.mouse_hotkeys.clone());
                    }
                }
            }

            // 分区自动吸附同类文件：后台轮询（2 秒一次；没有开启自动吸附的分区时几乎零开销）
            absorb::spawn_loop(app.handle().clone());

            // 分区窗口数量可能很多，而每个分区窗口都要新建一次 WebView2，属于启动最耗时的部分。
            // 这里不在 setup 里同步创建（会卡住首帧），而是从子线程投递到主线程事件循环：
            // 托盘图标/主窗口先就绪，分区窗口随后逐个出现。
            let args: Vec<String> = std::env::args().collect();
            let handle = app.handle().clone();
            // 退出守卫：独立进程盯着本进程，被强杀时也能把文件还回桌面；
            // 哨兵窗口负责接住 taskkill（不带 /f）/ 系统注销 / 关机的通知。
            session::install_exit_sentinel(handle.clone());
            std::thread::spawn(move || {
                session::spawn_guard();
            });
            std::thread::spawn(move || {
                if unclean_exit {
                    // 上次没走正常退出（守卫也没来得及/没生效）→ 这次先把文件还回桌面，
                    // 桌面必须是"看得见"的状态；下次启动再照常收回分区。
                    let db = handle.state::<Db>().inner().clone();
                    let _ = engine::sanitize_move_history(&db);
                    match engine::return_all_to_desktop(&db) {
                        Ok(0) => {}
                        Ok(n) => {
                            log_msg(&format!("非正常退出后归还文件：{n} 个"));
                            shellicon::refresh_desktop_icons();
                        }
                        Err(e) => log_msg(&format!("return_all_to_desktop (unclean) failed: {e}")),
                    }
                    session::mark_start();
                } else {
                    // 上次退出时文件都还回了桌面，这里按移动记录重新收进各分区。
                    // （上次要是崩溃退出、文件还在分区里，这一步自然什么都不做）
                    let db = handle.state::<Db>().inner().clone();
                    // 先修历史里的脏来源（老版本把跨分区搬运的 from 记成了另一个分区的
                    // 隐藏目录），再收回 —— 顺序反了的话收回/归还还会按脏地址搬文件。
                    match engine::sanitize_move_history(&db) {
                        Ok(0) => {}
                        Ok(n) => log_msg(&format!("修正搬运记录：{n} 条")),
                        Err(e) => log_msg(&format!("sanitize_move_history failed: {e}")),
                    }
                    match engine::reorganize_from_history(&db) {
                        Ok(0) => {}
                        Ok(n) => {
                            log_msg(&format!("启动收回文件：{n} 个"));
                            // 文件刚被搬进隐藏的分区文件夹，桌面自己不会立刻重画
                            shellicon::refresh_desktop_icons();
                        }
                        Err(e) => log_msg(&format!("reorganize_from_history failed: {e}")),
                    }
                }
                let h = handle.clone();
                if let Err(e) = handle.run_on_main_thread(move || {
                    let db = h.state::<Db>().inner().clone();
                    handle_add_box_args(&h, &db, &args);
                    if let Err(e) = boxwin::open_all_box_windows(&h, &db) {
                        log_msg(&format!("open_all_box_windows failed: {e}"));
                    }
                    // 便签组件：按上次的开关状态恢复窗口
                    notes::restore_on_startup(&h, &db);
                    // 股票面板组件：同上
                    stock::restore_on_startup(&h, &db);
                    // 黄历组件：同上
                    huangli::restore_on_startup(&h, &db);
                    // 诗词组件：同上
                    poetry::restore_on_startup(&h, &db);
                    // 黄帝内经组件：同上
                    huangdi::restore_on_startup(&h, &db);
                    // 伤寒论组件：同上
                    shanghan::restore_on_startup(&h, &db);
                }) {
                    log_msg(&format!("defer box windows failed: {e}"));
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_desktop_files,
            commands::get_boxes,
            commands::get_box_by_id,
            commands::get_box_icons,
            commands::create_box,
            commands::delete_box,
            commands::rename_box,
            commands::merge_boxes,
            commands::organize_desktop,
            commands::organize_desktop_plan,
            commands::organize_desktop_commit,
            commands::undo_last_organize,
            commands::get_last_organize_batch,
            commands::organize_box,
            commands::get_absorb_cfg,
            commands::set_absorb_cfg,
            commands::absorb_box_now,
            commands::get_rules,
            commands::save_rule,
            commands::delete_rule,
            commands::get_settings,
            commands::save_settings,
            commands::get_box_files,
            commands::get_box_window_cfg,
            commands::save_box_window_cfg,
            commands::set_box_visible,
            commands::set_box_collapsed,
            commands::tray_menu_action,
            commands::tray_menu_ping,
            commands::tray_menu_dismiss,
            commands::set_auto_start_enabled,
            commands::get_box_snap_targets,
            commands::start_box_drag,
            commands::set_box_link,
            commands::set_box_sort,
            commands::export_layout,
            commands::import_layout,
            commands::install_shell_menu,
            commands::uninstall_shell_menu,
            commands::is_shell_menu_installed,
            commands::get_shortcut_arrow_hidden,
            commands::set_shortcut_arrow_hidden,
            commands::apply_shortcut_arrow_forced,
            commands::open_path,
            commands::show_file_menu,
            commands::show_box_background_menu,
            commands::rename_file,
            commands::open_box_menu,
            commands::fit_box_menu,
            commands::close_box_menu,
            commands::box_menu_action,
            commands::open_widget_menu,
            commands::fit_widget_menu,
            commands::close_widget_menu,
            commands::widget_menu_publish,
            commands::widget_menu_get,
            commands::widget_menu_request,
            commands::widget_menu_pick,
            commands::start_file_drag,
            commands::get_notes,
            commands::add_note,
            commands::save_note,
            commands::delete_note,
            commands::list_note_tabs,
            commands::add_note_tab,
            commands::rename_note_tab,
            commands::delete_note_tab,
            commands::get_notes_enabled,
            commands::set_notes_enabled,
            commands::get_note_window_cfg,
            commands::save_note_window_cfg,
            commands::start_note_drag,
            commands::get_stocks,
            commands::add_stock,
            commands::delete_stock,
            commands::reorder_stocks,
            commands::fetch_stocks,
            commands::get_stocks_enabled,
            commands::set_stocks_enabled,
            commands::get_stock_window_cfg,
            commands::save_stock_window_cfg,
            commands::start_stock_drag,
            commands::get_huangli_enabled,
            commands::set_huangli_enabled,
            commands::get_huangli_window_cfg,
            commands::save_huangli_window_cfg,
            commands::start_huangli_drag,
            commands::get_poetry_enabled,
            commands::set_poetry_enabled,
            commands::get_poetry_window_cfg,
            commands::save_poetry_window_cfg,
            commands::start_poetry_drag,
            commands::get_huangdi_enabled,
            commands::set_huangdi_enabled,
            commands::get_huangdi_window_cfg,
            commands::save_huangdi_window_cfg,
            commands::start_huangdi_drag,
            commands::get_shanghan_enabled,
            commands::set_shanghan_enabled,
            commands::get_shanghan_window_cfg,
            commands::save_shanghan_window_cfg,
            commands::start_shanghan_drag,
            commands::search_files,
            commands::read_clipboard,
            commands::rebuild_search_index,
            commands::clear_index,
            commands::list_drives,
            commands::start_mouse_capture
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 分区窗口（box-*）有各自的 on_window_event：关闭前先持久化几何再 hide，
                // 这里交给它处理；其余窗口统一「拦截关闭 + 隐藏」。
                if window.label().starts_with("box-") {
                    return;
                }
                api.prevent_close();
                // 桌面组件/分区的隐藏必须走 hide_win 放行（见 boxwin::hide_win），
                // 否则会被"显示桌面免疫"的兜底补显示弹回来；主窗口、搜索面板
                // 不是桌面组件，普通 hide 即可。
                if boxwin::is_desktop_widget(window.label()) {
                    boxwin::hide_window(window);
                } else {
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("Qidomin 启动失败")
        .run(|app_handle, event| {
            // 兜住其它退出方式（系统注销/关机等）：退出前把分区文件归还桌面。
            // 托盘「退出」那条路已经做过一次，这里再跑一遍也安全（幂等）。
            match event {
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
                    // **退出标记先销、重活后干**（见 session::graceful_exit 的说明）：
                    // 关机/注销时系统给的时间很短，几何落盘 + 归还几十个文件很可能跑不完
                    // 就被杀掉。标记要是留到最后删，下次启动就会把这次退出判成"崩溃"，
                    // 走归还桌面那条路，开机后图标收不回分区。
                    session::mark_clean_exit();
                    // 任何退出方式（托盘/注销/关机）都先把分区几何强制落盘，再归还文件
                    boxwin::persist_all_geometry(app_handle);
                    if let Some(db) = app_handle.try_state::<Db>() {
                        match engine::return_all_to_desktop(&db) {
                            Ok(n) => log_msg(&format!("退出归还文件：{n} 个")),
                            Err(e) => log_msg(&format!("return_all_to_desktop failed: {e}")),
                        }
                        shellicon::refresh_desktop_icons();
                    }
                }
                _ => {}
            }
        });
}
