//! 原生 Shell 集成：打开文件 + 弹出与资源管理器完全一致的右键菜单。
//!
//! 为什么要自己调 Shell，而不是用 opener 插件 + 前端自定义菜单：
//!
//! 1) **打开文件**：opener 插件的 `open_path` 命令受 Tauri 权限（ACL）管控，
//!    能力声明里少一条 `allow-open-path` 就会被直接拒绝，前端只看到"双击没反应"。
//!    这里直接调 `ShellExecuteW`，没有任何权限门槛。
//!
//! 2) **右键菜单**：文件上右键用系统原生菜单（`IContextMenu`），打开方式 / 复制 /
//!    剪切 / 删除 / 发送到… 一样不少。
//!    分区**空白处**只保留三条我们自己能保证可用的：新建文件夹 / 新建文本文档 / 属性。
//!    系统背景菜单里剩下的是「用某某打开」「在终端中打开」「授予访问权限」这类
//!    第三方扩展项和 Explorer 视图自己的项，分区场景用不上，已整条去掉 ——
//!    既让菜单干净，也免掉每次都要取一遍 Shell 背景菜单的失败面。

use windows::core::{w, PCSTR, PCWSTR, HSTRING};
use windows::Win32::Foundation::{HANDLE, HWND, POINT};
use windows::Win32::System::Com::{CoInitializeEx, IBindCtx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{
    IContextMenu, IShellItem, SHCreateItemFromParsingName, SHObjectProperties, ShellExecuteW,
    BHID_SFUIObject, CMINVOKECOMMANDINFO, CMF_NORMAL, SHOP_FILEPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, InsertMenuW, TrackPopupMenuEx, MF_BYPOSITION,
    MF_SEPARATOR, MF_STRING, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON,
};

/// 我们补进原生菜单里的命令 id。
///
/// 必须落在 Shell 的 id 区间之外：`QueryContextMenu` 用的是 `[1, 0x7FFF]`，
/// 这里取 0x9000 起，绝不会和「打开 / 复制 / 删除…」撞号。
const CMD_RENAME: usize = 0x9000;
/// 下面三个只在**分区空白处**那份自建菜单里用
const CMD_NEW_FOLDER: usize = 0x9001;
const CMD_NEW_FILE: usize = 0x9002;
const CMD_PROPERTIES: usize = 0x9003;

/// 用户选了「重命名」时，返回给前端的动作名
pub const ACTION_RENAME: &str = "rename";

/// 用系统默认程序打开文件 / 文件夹（等价于在资源管理器里双击）。
///
/// 直接走 `ShellExecuteW`，不经过任何插件/权限层，所以不会出现"调用被拒→静默失败"。
pub fn open_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("路径为空".to_string());
    }
    let file = HSTRING::from(path);
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute 的约定：返回值 > 32 才算成功，<= 32 是错误码
    let code = r.0 as isize;
    if code <= 32 {
        return Err(format!("打开失败（ShellExecute 错误码 {code}）"));
    }
    Ok(())
}

/// 在屏幕光标处弹出该路径的系统右键菜单（和桌面/资源管理器里右键一模一样）。
///
/// - `hwnd` 用作菜单的宿主窗口；菜单是模态的，用户选完或点别处才返回。
/// - 菜单项 id 从 1 开始（`QueryContextMenu` 的 `idcmdfirst`），
///   回调时再减 1 变成"零基命令偏移"，用 `MAKEINTRESOURCE` 的方式塞进 `lpVerb`。
/// - 返回 `Some(ACTION_RENAME)` 表示用户点了我们补的「重命名」，需要调用方就地改名；
///   返回 `None` 表示用户取消、或命令已由 Shell 自己执行完。
pub fn show_context_menu(hwnd: HWND, path: &str) -> Result<Option<String>, String> {
    if path.trim().is_empty() {
        return Err("路径为空".to_string());
    }
    unsafe {
        // Shell 的右键菜单依赖 COM；线程可能已初始化过，忽略重复初始化
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let hstring = HSTRING::from(path);
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(hstring.as_ptr()), None)
            .map_err(|e| format!("找不到该项：{e}"))?;
        let ctx: IContextMenu = item
            .BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject)
            .map_err(|e| format!("无法获取右键菜单：{e}"))?;

        let menu = CreatePopupMenu().map_err(|e| e.to_string())?;
        if ctx.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL).is_err() {
            let _ = DestroyMenu(menu);
            return Err("生成右键菜单失败".to_string());
        }

        // Shell 给"单个文件"的 IContextMenu 里**没有「重命名」** —— 重命名是资源管理器
        // 视图自己的功能（由视图负责就地编辑），任何自己弹这个菜单的宿主都拿不到它。
        // 所以在这里补一条我们自己的，放最上面，选完由前端就地改名。
        let _ = InsertMenuW(
            menu,
            0,
            MF_BYPOSITION | MF_STRING,
            CMD_RENAME,
            w!("重命名"),
        );
        let _ = InsertMenuW(menu, 1, MF_BYPOSITION | MF_SEPARATOR, 0, PCWSTR::null());

        let picked = track_menu_at_cursor(menu, hwnd);
        let _ = DestroyMenu(menu);
        invoke_picked(&ctx, hwnd, picked)
    }
}

/// 在屏幕光标处弹出**分区空白处**的右键菜单。
///
/// 这里**不再去取系统背景菜单**。原因（都是本机实测的结论，记下来免得再走回头路）：
///
/// - 网上流传的空白处写法 `IShellFolder::GetUIObjectOf(hwnd, cidl = 0, apidl = NULL,
///   IID_IContextMenu)` 在真实 Shell 上恒返回 `E_INVALIDARG (0x80070057)`，
///   前端只会看到一句"操作失败"；Explorer 用的是 `CreateViewObject`，能拿到菜单，
///   但菜单里剩下的只有第三方扩展项（用 Visual Studio 打开 / 在终端中打开 /
///   授予访问权限…）和「属性」—— 资源管理器自己的「查看 / 排序方式 / 刷新 / 粘贴」
///   属于 DefView，任何自弹菜单的宿主都拿不到；系统的「新建」子菜单则要靠宿主转发
///   `WM_INITMENUPOPUP` 懒加载，在 WebView 宿主里点下去没反应。
/// - 也就是说，**能拿到的那部分恰恰不是我们要的**。分区空白处只需要做三件事，
///   而这三件事我们都能独立完成、行为完全可控。
///
/// 所以现在只画一份自建菜单：新建文件夹 / 新建文本文档 / ─── / 属性。
/// 不调 Shell、不依赖扩展项、没有失败分支，右键永远有反应。
pub fn show_folder_background_menu(hwnd: HWND, folder_path: &str) -> Result<(), String> {
    if folder_path.trim().is_empty() {
        return Err("路径为空".to_string());
    }
    unsafe {
        let menu = CreatePopupMenu().map_err(|e| e.to_string())?;
        let mut ok = true;
        ok &= InsertMenuW(menu, 0, MF_BYPOSITION | MF_STRING, CMD_NEW_FOLDER, w!("新建文件夹")).is_ok();
        ok &= InsertMenuW(menu, 1, MF_BYPOSITION | MF_STRING, CMD_NEW_FILE, w!("新建文本文档")).is_ok();
        ok &= InsertMenuW(menu, 2, MF_BYPOSITION | MF_SEPARATOR, 0, PCWSTR::null()).is_ok();
        ok &= InsertMenuW(menu, 3, MF_BYPOSITION | MF_STRING, CMD_PROPERTIES, w!("属性")).is_ok();
        if !ok {
            let _ = DestroyMenu(menu);
            crate::log_msg("bgmenu: 菜单项插入失败");
            return Err("生成右键菜单失败".to_string());
        }

        let picked = track_menu_at_cursor(menu, hwnd);
        let _ = DestroyMenu(menu);

        match picked {
            CMD_NEW_FOLDER => create_new_item(folder_path, true),
            CMD_NEW_FILE => create_new_item(folder_path, false),
            CMD_PROPERTIES => show_properties(hwnd, folder_path),
            // 0 = 点到菜单外 / Esc
            _ => Ok(()),
        }
    }
}

/// 弹出该路径的「属性」对话框（等价于资源管理器里右键 → 属性）。
///
/// 主路径 `SHObjectProperties(SHOP_FILEPATH)` 就是 Shell 官方给宿主用的接口；
/// 万一它失败（个别精简系统 / 被策略拦），退回 `ShellExecuteW` 的 `properties` 动词。
/// 两条都不通才报错 —— 但不会再把 COM 的 HRESULT 原文甩给用户。
fn show_properties(hwnd: HWND, path: &str) -> Result<(), String> {
    let h = HSTRING::from(path);
    let ok = unsafe {
        SHObjectProperties(
            Some(hwnd),
            SHOP_FILEPATH,
            PCWSTR(h.as_ptr()),
            PCWSTR::null(),
        )
    };
    if ok.as_bool() {
        return Ok(());
    }
    crate::log_msg("bgmenu: SHObjectProperties 失败，退回 ShellExecute properties");

    let r = unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("properties"),
            PCWSTR(h.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (r.0 as isize) <= 32 {
        crate::log_msg(&format!("bgmenu: 属性打开失败 code={}", r.0 as isize));
        return Err("打开属性失败".to_string());
    }
    Ok(())
}

/// 在 `dir` 里创建一个"新建文件夹"或"新建文本文档.txt"，重名则加 " (2)"、" (3)"…
fn create_new_item(dir: &str, is_folder: bool) -> Result<(), String> {
    let base = if is_folder { "新建文件夹" } else { "新建文本文档.txt" };
    let path = std::path::Path::new(dir);
    let target = crate::engine::unique_target(path, base);
    if is_folder {
        std::fs::create_dir(&target).map_err(|e| format!("创建文件夹失败：{e}"))?;
    } else {
        std::fs::write(&target, []).map_err(|e| format!("创建文件失败：{e}"))?;
    }
    crate::log_msg(&format!(
        "bgmenu: 新建{}成功 {}",
        if is_folder { "文件夹" } else { "文件" },
        target.to_string_lossy()
    ));
    Ok(())
}

/// 在屏幕光标处弹出 `menu`，返回被选中的菜单项 id（0 = 取消）。
///
/// z 序处理：分区窗口平时钉在桌面层（z 序最低档），而 `TrackPopupMenuEx`
/// 弹出的菜单跟着宿主的档位走 —— 撞上展开的饼型文件（TOPMOST 档）就被盖住，
/// 表现为「右键菜单一会在上、一会文件在上」。这里先把宿主临时抬到 TOPMOST 档
/// （菜单随宿主出现在最上层），菜单关闭后按原样放回（由命令层的沉底逻辑归位）。
unsafe fn track_menu_at_cursor(menu: windows::Win32::UI::WindowsAndMessaging::HMENU, hwnd: HWND) -> usize {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, WS_EX_TOPMOST,
    };
    let mut pt = POINT { x: 0, y: 0 };
    let _ = GetCursorPos(&mut pt);
    // 只在宿主本来不是置顶时才动它：展开中的饼型窗口自己就是 TOPMOST，
    // 菜单关了不能顺手把人家的置顶摘掉
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let was_topmost = ex & WS_EX_TOPMOST.0 != 0;
    if !was_topmost {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    // TPM_RETURNCMD：菜单不给我们发命令消息，而是把选中的 id 直接当返回值返回，
    // 这样不用额外挂消息钩子 / 跑消息循环。
    let picked = TrackPopupMenuEx(
        menu,
        TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0,
        pt.x,
        pt.y,
        hwnd,
        None,
    );
    if !was_topmost {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    picked.0 as usize
}

/// 处理用户选中的菜单项。
///
/// `picked == 0` → 取消；`picked == CMD_RENAME` → 返回 `Some(ACTION_RENAME)`；
/// 其余 → 调 `InvokeCommand` 让 Shell 执行系统命令（复制 / 粘贴 / 删除 / 打开方式…）。
unsafe fn invoke_picked(ctx: &IContextMenu, hwnd: HWND, picked: usize) -> Result<Option<String>, String> {
    if picked == 0 {
        // 0 = 用户点了菜单外/按了 Esc
        return Ok(None);
    }
    if picked == CMD_RENAME {
        // 我们自己那条：不改文件，把意图交回前端做就地改名
        return Ok(Some(ACTION_RENAME.to_string()));
    }

    let info = CMINVOKECOMMANDINFO {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
        fMask: 0,
        hwnd,
        // MAKEINTRESOURCE(id - 1)：高位为 0 时，Shell 才知道这是"命令偏移"而不是字符串
        lpVerb: PCSTR((picked - 1) as *const u8),
        lpParameters: PCSTR::null(),
        lpDirectory: PCSTR::null(),
        nShow: SW_SHOWNORMAL.0,
        dwHotKey: 0,
        hIcon: HANDLE::default(),
    };
    ctx.InvokeCommand(&info)
        .map_err(|e| format!("执行右键菜单命令失败：{e}"))?;
    Ok(None)
}
