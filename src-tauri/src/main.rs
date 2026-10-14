#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // 退出守卫模式：`qidomin_guard.exe --guard <主进程 pid>`
    // （主进程自己复制出来的副本，进程名不同，所以 `taskkill /f /im qidomin.exe` 杀不到它）
    if let Some(pos) = args.iter().position(|a| a == "--guard") {
        let pid = args
            .get(pos + 1)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        qidomin_lib::run_exit_guard(pid);
        return;
    }
    // 单实例检测放在最前面：已有健康实例时立刻退出，既最快，也避免误清理其 WebView2。
    if qidomin_lib::check_single_instance() {
        return;
    }
    // 清理本应用残留的 WebView2 进程（防止新实例/新窗口初始化挂起）。
    // 只有确定要接管启动的实例才执行，因此不会影响正在运行的健康实例。
    qidomin_lib::cleanup_stale_webview2();
    // 清理无窗口的旧实例（减少托盘重复图标）
    qidomin_lib::kill_stale_instances();
    qidomin_lib::run();
}
