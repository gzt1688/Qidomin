//! 系统剪贴板读取（搜索面板「粘贴」按钮用）。
//!
//! 只做一件事：告诉前端"剪贴板里现在是什么"，**自己不做任何搬运**。
//! 三类结果，按优先级：
//!
//! 1. `text` —— 有文字，直接用（从文档 / 网页 / 聊天窗口里复制的场景）；
//! 2. `files` —— 只有文件（在资源管理器里 Ctrl+C 复制的文件 / 文件夹），
//!    只回文件名给前端显示提示用；**绝不把文件搬去任何地方** ——
//!    搜索框是"搜"，不是"放文件的地方"，放文件请直接拖进分区；
//! 3. `empty` —— 都没有。
//!
//! **任何情况都不向上返回错误**。剪贴板为空、剪贴板里只有图片、被别的程序
//! （远程桌面 / 剪贴板管理器）占着读不出来 —— 这些都不是"故障"，只是"没内容"。
//! 以前这里把底层的技术性错误当成 Err 抛给前端，用户剪贴板空着点一下「粘贴」
//! 就会看到一个"操作失败：…"的弹框，非常突兀。

use std::path::Path;

use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// 一次最多带几个文件名进搜索框。多选几十个文件时，把名字全塞进去既放不下
/// 也搜不准，取前几个足够表达"我要找这些"。
const MAX_NAMES: u32 = 6;

/// 剪贴板现在的内容（给前端判断"能不能拿去搜"）。
///
/// `kind` 三选一：`"text"`（有文字）/ `"files"`（只有文件）/ `"empty"`（都没有）。
/// `files` 只在 `kind == "files"` 时有值，且最多 `MAX_NAMES` 个 —— 只用于提示，
/// 不参与搜索。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardInfo {
    /// `"text"` / `"files"` / `"empty"`
    pub kind: &'static str,
    /// `kind == "text"` 时的文字内容
    pub text: String,
    /// `kind == "files"` 时的文件名（不含路径）
    pub files: Vec<String>,
}

/// 读一次剪贴板并分类。**永远不返回错误**：读不到就当 `empty`。
pub fn info() -> ClipboardInfo {
    if let Some(t) = text() {
        let t = t.trim();
        if !t.is_empty() {
            return ClipboardInfo {
                kind: "text",
                text: t.to_string(),
                files: Vec::new(),
            };
        }
    }
    let names = file_names();
    if names.is_empty() {
        return ClipboardInfo {
            kind: "empty",
            text: String::new(),
            files: Vec::new(),
        };
    }
    ClipboardInfo {
        kind: "files",
        text: String::new(),
        files: names,
    }
}

/// 剪贴板里的文字；读不到（空 / 只有文件 / 被占用）一律当作"没有文字"。
///
/// 跨进程和格式细节交给 arboard，我们只关心"有没有一段能搜的字"。
fn text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// 剪贴板里的文件列表（CF_HDROP）→ 文件名（不含路径）。
///
/// 搜索是按**文件名**匹配的，所以给文件名而不是完整路径；
/// 名字里带空格也没关系，前端会原样填进搜索框。
fn file_names() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // SAFETY: 只在 OpenClipboard 成功后访问剪贴板，且所有分支都会 CloseClipboard；
    // DragQueryFileW 拿到的是本次打开的剪贴板句柄，句柄在关闭前一直有效。
    unsafe {
        if OpenClipboard(None).is_err() {
            // 剪贴板被别的程序占着 —— 当"没内容"处理，不报错
            return out;
        }
        if let Ok(handle) = GetClipboardData(CF_HDROP.0 as u32) {
            let drop = HDROP(handle.0);
            // ifile = 0xFFFFFFFF 时返回文件个数
            let count = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..count.min(MAX_NAMES) {
                let len = DragQueryFileW(drop, i, None);
                if len == 0 {
                    continue;
                }
                // len 不含结尾的 0，所以缓冲区开 len + 1
                let mut buf = vec![0u16; len as usize + 1];
                let n = DragQueryFileW(drop, i, Some(&mut buf));
                if n == 0 {
                    continue;
                }
                let full = String::from_utf16_lossy(&buf[..n as usize]);
                let name = Path::new(&full)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or(full);
                let name = name.trim().to_string();
                if !name.is_empty() && !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        let _ = CloseClipboard();
    }
    out
}
