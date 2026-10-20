//! 退出守卫：**不管程序怎么没的，桌面的文件都得回到桌面上。**
//!
//! ## 问题
//!
//! 分区里的文件是从桌面搬进隐藏分区文件夹的，正常退出时会全部搬回桌面
//! （`engine::return_all_to_desktop`）。但下面这些退出方式**根本不给程序留时间**：
//!
//! - 任务管理器 / `taskkill /f` 直接 `TerminateProcess`；
//! - 崩溃、断电、系统强杀。
//!
//! 结果是：程序没了，文件还躺在隐藏的分区文件夹里 —— 桌面上什么都没有。
//!
//! ## 三道防线
//!
//! 1. **会话标记**（`session.json`）：程序起来就写，正常退出就删。
//!    下次启动时若是"标记还在"，说明上次没走正常退出流程 —— 启动时先把文件还回桌面。
//! 2. **独立守卫进程**：启动时把自己复制一份成 `qidomin_guard.exe`（进程名不同，
//!    `taskkill /f /im qidomin.exe` 杀不到它），它只做一件事 —— 盯着主进程，
//!    主进程一没（崩溃 / 被强杀都算）就去检查标记，还在就把文件还回桌面。
//!    这样**被强杀的那一刻桌面就恢复了**，不用等下次启动。
//! 3. **隐藏哨兵窗口**：`taskkill`（不带 /f）、系统注销 / 关机发的是
//!    `WM_CLOSE` / `WM_ENDSESSION`，哨兵窗口收到后走一遍完整的"正常退出"，
//!    文件还回桌面、几何落盘，再退出进程。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_ENDSESSION, WM_QUERYENDSESSION,
    WNDCLASSW, WS_OVERLAPPED,
};

/// 应用数据目录（%APPDATA%\com.qidomin.app）
fn app_dir() -> Option<PathBuf> {
    let d = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(d).join("com.qidomin.app"))
}

/// 会话标记文件路径
fn session_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("session.json"))
}

/// 守卫进程的可执行文件（自己的一份副本，放在应用数据目录里）
fn guard_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("qidomin_guard.exe"))
}

/// 本实例开始接管桌面：写一份会话标记。
///
/// 必须在**拿到单实例锁之后**调用，否则第二个实例会把正在运行的那个实例的
/// 标记覆盖成自己的 pid。
pub fn mark_start() {
    let Some(p) = session_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = format!("{{\"pid\":{}}}", std::process::id());
    let _ = std::fs::write(&p, body);
}

/// 正常退出：删掉标记。
///
/// 守卫进程醒了以后看到标记没了，就知道文件已经还过了，直接退出。
pub fn mark_clean_exit() {
    if let Some(p) = session_path() {
        let _ = std::fs::remove_file(&p);
    }
    // 顺手清掉历次认领出来的残留（认领时改名留下的）
    if let Some(dir) = app_dir() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("session.claimed.") {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

/// 原子地"认领"上一份残留标记。
///
/// 返回 `true` = 上次不是正常退出（崩溃 / 被强杀 / 断电），需要把文件还回桌面。
///
/// 用**改名**而不是"读 + 删"：改名在 Windows 上是原子的，
/// 守卫进程和刚启动的新实例同时来认领时也只会有一个成功，不会两边各搬一遍。
pub fn claim_stale_session() -> bool {
    let (Some(src), Some(dir)) = (session_path(), app_dir()) else {
        return false;
    };
    if !src.exists() {
        return false;
    }
    let dst = dir.join(format!("session.claimed.{}", std::process::id()));
    let _ = std::fs::remove_file(&dst);
    std::fs::rename(&src, &dst).is_ok()
}

/// 认领出来的那份标记用完了就删掉（下次别再认一遍）
fn drop_claim() {
    if let Some(dir) = app_dir() {
        let f = dir.join(format!("session.claimed.{}", std::process::id()));
        let _ = std::fs::remove_file(&f);
    }
}

/// 把分区里的文件全部还回桌面（守卫进程和启动自愈共用）。
///
/// 刻意不依赖 Tauri 的 AppHandle：守卫进程是独立跑的，拿不到句柄，
/// 直接按应用数据目录打开同一个库。
fn restore_desktop_files(why: &str) {
    let Some(dir) = app_dir() else { return };
    let db = match crate::db::Db::open(&dir.join("qidomin.db")) {
        Ok(d) => d,
        Err(e) => {
            crate::log_msg(&format!("{why}: 打开数据库失败：{e}"));
            return;
        }
    };
    // 先修历史里的脏来源，再归还，顺序反了会按脏地址把文件搬进看不见的目录
    let _ = crate::engine::sanitize_move_history(&db);
    match crate::engine::return_all_to_desktop(&db) {
        Ok(0) => crate::log_msg(&format!("{why}: 没有需要归还的文件")),
        Ok(n) => {
            crate::log_msg(&format!("{why}: 已归还 {n} 个文件到桌面"));
            // 让桌面立刻重画，不用手工 F5（轻量通知即可，不需要重建图标缓存）
            crate::shellicon::refresh_desktop_light();
        }
        Err(e) => crate::log_msg(&format!("{why}: 归还失败：{e}")),
    }
}

/// 启动独立守卫进程（复制自身 + 后台盯着主进程）。
///
/// 只在文件复制 / 启动失败时打日志，绝不影响主程序。
pub fn spawn_guard() {
    let Some(dst) = guard_path() else { return };
    let Ok(self_exe) = std::env::current_exe() else {
        return;
    };
    let self_len = std::fs::metadata(&self_exe).map(|m| m.len()).unwrap_or(0);
    let need_copy = match std::fs::metadata(&dst) {
        Ok(m) => m.len() != self_len,
        Err(_) => true,
    };
    if need_copy {
        if let Err(e) = std::fs::copy(&self_exe, &dst) {
            crate::log_msg(&format!("exit guard: 复制守卫程序失败：{e}"));
            return;
        }
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let pid = std::process::id().to_string();
    let r = std::process::Command::new(&dst)
        .args(["--guard", &pid])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match r {
        Ok(_) => crate::log_msg("exit guard: 守卫进程已启动"),
        Err(e) => crate::log_msg(&format!("exit guard: 守卫进程启动失败：{e}")),
    }
}

/// 守卫进程主体：`qidomin.exe --guard <主进程 pid>`。
///
/// 等主进程句柄有信号（正常退出 / 崩溃 / 被强杀都算），再决定要不要还文件。
pub fn run_exit_guard(parent_pid: u32) {
    if parent_pid > 0 && parent_pid != std::process::id() {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) {
                let _ = WaitForSingleObject(h, INFINITE);
                let _ = CloseHandle(h);
            }
        }
    }
    // 主进程刚死，磁盘上可能还有尾巴（写标记 / 收尾 IO），稍等一下再判断
    std::thread::sleep(std::time::Duration::from_millis(500));

    // 标记没了 = 正常退出过，或者新实例已经接手并处理了 → 什么都不做
    if !claim_stale_session() {
        return;
    }
    crate::log_msg("exit guard: 检测到上次不是正常退出，正在把文件还回桌面");
    restore_desktop_files("exit guard");
    drop_claim();
}

// ---------------------------------------------------------------------------
// 隐藏哨兵窗口：接住 WM_CLOSE / WM_ENDSESSION，走一遍完整的正常退出
// ---------------------------------------------------------------------------

static SENTINEL_APP: OnceLock<AppHandle> = OnceLock::new();
static EXITING: AtomicBool = AtomicBool::new(false);
/// 哨兵窗口的类名（也是窗口标题，排查时好认）
const SENTINEL_CLASS: &str = "QidominExitSentinel";

/// 建一个看不见的顶层窗口，专门收退出通知。
///
/// 放在**独立线程 + 自己的消息循环**里：窗口归这个线程所有，
/// 系统发过来的 `WM_ENDSESSION` / `WM_CLOSE` 由这个线程直接派发，
/// 不惊动 tao 的事件循环，也不受它阻塞。
pub fn install_exit_sentinel(app: AppHandle) {
    let _ = SENTINEL_APP.set(app);
    std::thread::Builder::new()
        .name("exit-sentinel".into())
        .spawn(|| unsafe {
            sentinel_thread();
        })
        .ok();
}

/// 完整的"正常退出"：**先销标记** → 几何落盘 → 尽力归还文件 → 结束进程。
///
/// 销标记必须排在最前面，理由见函数内注释。
fn graceful_exit(reason: &str) {
    // 一次退出只跑一遍：关机时多个窗口会各收一次 WM_ENDSESSION
    if EXITING.swap(true, Ordering::SeqCst) {
        return;
    }
    // **先销标记，再做重活**（顺序不能反）。
    //
    // 关机/注销留给进程的时间极短：下面的几何落盘 + 归还几十个文件常常跑到一半
    // 就被系统 TerminateProcess。标记要是留在最后删，那次退出就会被下次启动
    // 误判成"崩溃/被强杀" → 走「把文件还回桌面」那条路，开机自启后用户看到的
    // 是桌面上散落的图标，非得手动重启一次程序才会被收回分区。
    //
    // 先销标记后，本次退还**尽力而为**：跑完了文件就在桌面（下次启动照常收回），
    // 被打断了文件就还在分区里（下次启动同样无事发生，分区窗口一开就看得见）。
    mark_clean_exit();
    crate::log_msg(&format!("exit sentinel: 收到{reason}，开始正常退出"));
    if let Some(app) = SENTINEL_APP.get() {
        crate::boxwin::persist_all_geometry(app);
        if let Some(db) = app.try_state::<crate::db::Db>() {
            restore_desktop_files_with(db.inner(), "退出归还");
        }
    }
    // 标记早在函数开头就销掉了（见上），这里不再重复
    std::process::exit(0);
}

/// 用现成的 Db 句柄归还（正常路径），省去再开一次库
fn restore_desktop_files_with(db: &crate::db::Db, why: &str) {
    let _ = crate::engine::sanitize_move_history(db);
    match crate::engine::return_all_to_desktop(db) {
        Ok(n) => {
            crate::log_msg(&format!("{why}: {n} 个文件已回到桌面"));
            if n > 0 {
                crate::shellicon::refresh_desktop_light();
            }
        }
        Err(e) => crate::log_msg(&format!("{why}: 失败：{e}")),
    }
}

/// 哨兵窗口过程
unsafe extern "system" fn sentinel_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // 系统问"能不能结束"：我们有事要做，但一定放行（返回 TRUE），
        // 真正的收尾放在 WM_ENDSESSION 里做
        //
        // 这里**先把退出标记销掉**：WM_QUERYENDSESSION 是最早的关机/注销通知，
        // 比 WM_ENDSESSION 更不容易被强杀截断。销了之后哪怕后面一步都没跑成，
        // 下次启动也会按搬运记录正常收回分区，而不是把文件全撒回桌面。
        // （用户中途取消关机也没关系：标记本来就该在退出时才清，程序还活着。）
        WM_QUERYENDSESSION => {
            mark_clean_exit();
            LRESULT(1)
        }
        // 真的要结束了（wParam != 0 表示不是"取消关机"）
        WM_ENDSESSION => {
            if wparam.0 != 0 {
                graceful_exit("系统注销/关机通知");
            }
            LRESULT(0)
        }
        // taskkill（不带 /f）给每个顶层窗口发 WM_CLOSE
        WM_CLOSE => {
            graceful_exit("关闭通知");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 建窗 + 自己的消息循环（跑在独立线程上，见 [`install_exit_sentinel`]）
unsafe fn sentinel_thread() {
    let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
        .unwrap_or_default();
    let class = windows::core::HSTRING::from(SENTINEL_CLASS);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(sentinel_proc),
        hInstance: hinstance.into(),
        lpszClassName: windows::core::PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        crate::log_msg("exit sentinel: 注册窗口类失败，退出守卫不可用");
        return;
    }
    let title = windows::core::HSTRING::from(SENTINEL_CLASS);
    // 故意**不**给 WS_VISIBLE：看不见，但仍是顶层窗口，收得到广播
    let hwnd = match CreateWindowExW(
        WINDOW_EX_STYLE(0),
        windows::core::PCWSTR(class.as_ptr()),
        windows::core::PCWSTR(title.as_ptr()),
        WS_OVERLAPPED,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(hinstance.into()),
        None,
    ) {
        Ok(hwnd) => hwnd,
        Err(_) => {
            crate::log_msg("exit sentinel: 创建哨兵窗口失败，退出守卫不可用");
            return;
        }
    };
    // 句柄要一直留着：窗口属于本线程，句柄丢了也没关系，但留着好排查
    crate::log_msg(&format!("exit sentinel: 哨兵窗口已就绪（{:?}）", hwnd));
    let mut msg = std::mem::MaybeUninit::<windows::Win32::UI::WindowsAndMessaging::MSG>::uninit();
    loop {
        let r = GetMessageW(msg.as_mut_ptr(), None, 0, 0);
        if r.0 <= 0 {
            break;
        }
        let msg = msg.assume_init();
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    let _ = PostQuitMessage(0);
}

/// 调试用：清理守卫留下的认领文件（正常路径用不到，测试里避免互相干扰）
#[allow(dead_code)]
pub(crate) fn cleanup_claims(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("session.claimed.") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {

    /// 会话标记的基本生命周期：写 → 认领 → 认领第二次应该失败
    #[test]
    fn claim_is_exclusive() {
        let dir = std::env::temp_dir().join("qd_session_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        // 直接测改名语义（不依赖真实的 APPDATA）
        let src = dir.join("session.json");
        std::fs::write(&src, "{\"pid\":1}").unwrap();
        let dst = dir.join(format!("session.claimed.{}", std::process::id()));
        let _ = std::fs::remove_file(&dst);
        assert!(std::fs::rename(&src, &dst).is_ok(), "第一次认领应当成功");
        assert!(!src.exists(), "认领后原标记应当不在了");
        assert!(
            std::fs::rename(&src, &dst).is_err(),
            "第二次认领应当失败（源已不存在）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
