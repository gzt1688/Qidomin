//! 从分区里把文件"拖出去"：走 Windows 原生 OLE 拖放。
//!
//! 用系统原生方式的好处是拖的是**真文件**：可以落到桌面、资源管理器窗口、别的分区，
//! 甚至别的程序（微信 / QQ / 邮件…）里，和资源管理器里拖一个文件完全一样。
//!
//! **数据对象怎么造 —— 这是"拖动时显示红色禁止圈"的根源**：
//! 不能用 `SHCreateDataObject`(NULL, cidl=1, [绝对 PIDL])。它要求 `apidl` 是**相对
//! `pidlFolder` 的子 PIDL**，而 `SHParseDisplayName` 给的是绝对 PIDL，塞进去得到的是
//! 一个格式不完整的数据对象 —— 于是所有落点（桌面 / 资源管理器 / 本程序窗口）都判定
//! "拖的东西读不出来"，光标全程是禁止圈。
//!
//! 正确做法是走 Shell 自己的路子：`IShellItem` → `IShellItemArray` →
//! `BindToHandler(BHID_DataObject)`。这正是资源管理器拖文件时构造的那个数据对象，
//! 格式（CF_HDROP / SHELLIDLIST）天然齐全，任何目标程序都认。
//!
//! **另外两个必须遵守的前提**（否则拖拽起不来或行为异常）：
//! 1. 调用线程必须先 `OleInitialize` —— 官方文档对 `DoDragDrop` 的硬性要求，
//!    只调 `CoInitializeEx` 不会初始化 OLE 的拖放子系统；
//! 2. 必须在**拥有该窗口的线程**（也就是 Tauri 主线程）上调用：拖放是个模态消息
//!    循环，要接管本线程的消息分发。所以上层命令
//!    [`crate::commands::start_file_drag`] 特意保持同步（同步命令才跑在主线程）。

use std::sync::Once;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{IBindCtx, IDataObject};
use windows::Win32::System::Ole::{
    IDropSource, OleInitialize, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE,
};
use windows::Win32::UI::Shell::{
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromShellItem, SHDoDragDrop,
    IShellItem, IShellItemArray, BHID_DataObject,
};

/// 进程级 OLE 初始化。
///
/// 只做一次：主线程和应用同生命周期，没必要也不应该在每次拖拽后 `OleUninitialize`
/// （引用计数容易配错，反而把别处依赖的 OLE 给关了）。
static OLE_INIT: Once = Once::new();

fn ensure_ole() {
    OLE_INIT.call_once(|| unsafe {
        // 已初始化过会返回 S_FALSE，同样可用
        let _ = OleInitialize(None);
    });
}

/// 以原生拖放方式拖出一个文件；**用户松开鼠标（拖放结束）后本函数才返回**。
///
/// `hwnd` 是拖放的宿主窗口，必须在**调用本函数的线程**上创建（见模块头说明），
/// 实际落点由光标下的目标程序决定。
pub fn start_drag(hwnd: HWND, path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("没有可拖动的文件".into());
    }
    ensure_ole();
    unsafe {
        let wide = HSTRING::from(path);
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
            .map_err(|e| format!("找不到要拖动的文件：{e}"))?;
        let arr: IShellItemArray =
            SHCreateShellItemArrayFromShellItem(&item).map_err(|e| format!("创建拖放数据失败：{e}"))?;
        // BHID_DataObject：资源管理器自己就是这么拿数据对象的
        let data: IDataObject = arr
            .BindToHandler(None::<&IBindCtx>, &BHID_DataObject)
            .map_err(|e| format!("创建拖放数据失败：{e}"))?;

        // 允许 复制 / 移动 / 创建快捷方式 —— 具体哪个由落点的目标程序决定
        let allowed = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0);
        SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, allowed)
            .map_err(|e| format!("拖放失败：{e}"))?;
    }
    Ok(())
}
