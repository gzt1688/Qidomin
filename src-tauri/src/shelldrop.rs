//! 分区窗口的拖放落点：自己接管窗口的 `IDropTarget`。
//!
//! ## 为什么非要自己接管
//!
//! tao（Tauri 用的窗口库）内置的落点处理器**只认 `CF_HDROP`**，数据对象里
//! 没有文件路径的一律回 `DROPEFFECT_NONE` —— 表现就是「此电脑 / 回收站 /
//! 控制面板 / 网络」这类桌面图标拖到分区上时光标直接变成禁止，松手什么都不会发生。
//!
//! 原因是这些东西**本来就不是文件**：它们在拖放时给的是 `CFSTR_SHELLIDLIST`
//! （一串 PIDL，shell 命名空间里的身份证），压根没有磁盘路径可给。
//!
//! 所以这里自己实现 `IDropTarget`，两种格式都认：
//!
//! - **有 `CF_HDROP`**（从桌面 / 资源管理器拖来的文件或文件夹）→ 照旧搬进分区；
//! - **只有 `CFSTR_SHELLIDLIST`**（虚拟项）→ 在分区目录里建一个指向该项的 `.lnk`
//!   （`IShellLinkW::SetIDList` 就是为这种非文件系统对象准备的），
//!   双击照样能打开「此电脑」「回收站」，图标也是系统原样。
//!
//! ## 一条安全红线
//!
//! 虚拟项**一律只建快捷方式，绝不搬运**。shell 命名空间里的东西解析出来的
//! 「路径」可能是盘符根目录（`C:\`）、控制面板一类的伪路径 —— 真去搬就出大事了。
//! 而且 `CF_HDROP` 存在时永远优先用它，只有它缺席时才走 shell 项这条支线，
//! 所以从桌面拖一个普通文件夹过来，行为跟以前完全一样（搬进去，不是建快捷方式）。

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Mutex;

use windows::core::{implement, w, Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, IDataObject, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP,
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::Shell::{
    CIDA, DragFinish, DragQueryFileW, HDROP, ILCombine, ILFree, ILGetSize, IShellLinkW, ShellLink,
    SHGetNameFromIDList, SIGDN_NORMALDISPLAY,
};

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

/// 一次拖放携带的内容
enum Payload {
    /// 真实文件路径（可以搬进分区）
    Files(Vec<PathBuf>),
    /// 虚拟 shell 项（只能建快捷方式）
    ShellItems(Vec<NamedPidl>),
    /// 都不认（拖进来的是文本、图片流…）
    Unsupported,
}

/// 一个 shell 命名空间项：显示名 + PIDL
struct NamedPidl {
    pidl: *mut ITEMIDLIST,
    name: String,
}

impl Drop for NamedPidl {
    fn drop(&mut self) {
        if !self.pidl.is_null() {
            unsafe { ILFree(Some(self.pidl)) };
        }
    }
}

/// 读 `CF_HDROP`：拖进来的真实文件 / 文件夹路径。
///
/// 拿不到（拖的不是文件）返回 `None`，好让调用方去试 shell 项那一路。
unsafe fn read_hdrop(data: &IDataObject) -> Option<Vec<PathBuf>> {
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium: STGMEDIUM = data.GetData(&fmt).ok()?;
    let hdrop = HDROP(medium.u.hGlobal.0);

    // 0xFFFFFFFF 表示"先问一共有几项"
    let count = DragQueryFileW(hdrop, u32::MAX, None);
    let mut out: Vec<PathBuf> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = DragQueryFileW(hdrop, i, None) as usize;
        if len == 0 {
            continue;
        }
        let mut buf = vec![0u16; len + 1];
        let n = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
        if n > 0 {
            use std::os::windows::ffi::OsStringExt;
            out.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
        }
    }
    DragFinish(hdrop);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 读 `CFSTR_SHELLIDLIST`：一串 PIDL。
///
/// 数据是一块 `CIDA`：父文件夹的绝对 PIDL + `cidl` 个相对 PIDL，
/// 用 `ILCombine` 拼成完整 PIDL 才能拿去问名字 / 建快捷方式。
unsafe fn read_shell_items(data: &IDataObject) -> Vec<NamedPidl> {
    let mut out: Vec<NamedPidl> = Vec::new();
    let cf = RegisterClipboardFormatW(w!("Shell IDList Array"));
    let fmt = FORMATETC {
        cfFormat: cf as u16,
        ptd: ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = data.GetData(&fmt) else {
        return out;
    };
    let ptr = GlobalLock(medium.u.hGlobal);
    if ptr.is_null() {
        ReleaseStgMedium(&mut medium);
        return out;
    }
    for pidl in parse_cida(ptr) {
        out.push(NamedPidl {
            pidl,
            name: pidl_name(pidl),
        });
    }
    let _ = GlobalUnlock(medium.u.hGlobal);
    ReleaseStgMedium(&mut medium);
    out
}

/// 从一块 `CIDA` 内存里解出每一项的**完整** PIDL（父 + 子拼起来）。
///
/// 返回的指针归调用方，用 `ILFree` 释放。
unsafe fn parse_cida(ptr: *const core::ffi::c_void) -> Vec<*mut ITEMIDLIST> {
    let mut out: Vec<*mut ITEMIDLIST> = Vec::new();
    if ptr.is_null() {
        return out;
    }
    // CIDA 是 packed 结构，字段不能直接取引用（未对齐），一律走 addr_of + read_unaligned
    let cida = ptr as *const CIDA;
    let cidl = ptr::addr_of!((*cida).cidl).read_unaligned() as usize;
    if cidl == 0 {
        return out;
    }
    // aoffset 实际有 cidl + 1 项（第 0 项是父 PIDL 的偏移），
    // 结构体里只声明了 1 项，按连续内存多读几项即可。
    let aoff = ptr::addr_of!((*cida).aoffset) as *const u32;
    let offsets = std::slice::from_raw_parts(aoff, cidl + 1);
    let parent = ptr
        .cast::<u8>()
        .add(offsets[0] as usize)
        .cast::<ITEMIDLIST>();
    for i in 0..cidl {
        let child = ptr
            .cast::<u8>()
            .add(offsets[i + 1] as usize)
            .cast::<ITEMIDLIST>();
        let full = ILCombine(Some(parent), Some(child));
        if !full.is_null() {
            out.push(full);
        }
    }
    out
}

/// PIDL 的显示名（"此电脑" / "回收站" / 磁盘名…）
unsafe fn pidl_name(pidl: *const ITEMIDLIST) -> String {
    SHGetNameFromIDList(pidl, SIGDN_NORMALDISPLAY)
        .ok()
        .and_then(|p| p.to_string().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_default()
}

/// 优先 `CF_HDROP`，没有再试 shell 项
unsafe fn read_payload(data: &IDataObject) -> Payload {
    let p = if let Some(files) = read_hdrop(data) {
        Payload::Files(files)
    } else {
        let items = read_shell_items(data);
        if items.is_empty() {
            Payload::Unsupported
        } else {
            Payload::ShellItems(items)
        }
    };
    // 排查用：拖进来的是什么、落点有没有认下它，日志里一眼能看到（节流防刷屏）
    log_throttled(&format!(
        "shelldrop: 读到 {}",
        match &p {
            Payload::Files(v) => format!("{} 个文件", v.len()),
            Payload::ShellItems(v) => format!(
                "{} 个虚拟项（{}）",
                v.len(),
                v.iter()
                    .map(|i| i.name.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            ),
            Payload::Unsupported => "无法识别的内容".to_string(),
        }
    ));
    p
}

/// 挑一个"源认可的"落点效果。
///
/// 这一步是"此电脑 / 回收站拖不进来"的关键：这些虚拟项**只能复制不能移动**，
/// 而 Explorer 拖它们时给的允许效果往往是 `LINK`（建快捷方式）或 `MOVE`，
/// 压根不含 `COPY`。以前只在 `want & allowed` 为空时退到 `COPY`，
/// 交集还是 0 → 我们回了 `DROPEFFECT_NONE` → 光标变成"禁止"，松手什么也不发生。
///
/// 现在按 想要 → LINK → COPY → MOVE → 源给什么都接 的顺序兜底：
/// 只要内容的确是我们要的（文件 / shell 项），就一定能拿到一个非空效果。
/// 收敛成**一个**效果位。
///
/// OLE 规定落点只能回一个 `DROPEFFECT` 位。回多位时，有的源（Explorer 就是）
/// 会当成"目标没表态"，直接把光标画成禁止 —— 所以最后必须只留一位。
/// 优先留"我们本来想要的"那个（虚拟项想 LINK，文件想 MOVE/COPY）。
fn single_effect(eff: u32, want: u32) -> u32 {
    let bits = [DROPEFFECT_LINK.0, DROPEFFECT_COPY.0, DROPEFFECT_MOVE.0];
    for b in bits {
        if eff & b != 0 && want & b != 0 {
            return b;
        }
    }
    for b in bits {
        if eff & b != 0 {
            return b;
        }
    }
    0
}

fn pick_effect(want: u32, allowed: u32) -> u32 {
    if want == 0 {
        return 0;
    }
    let mut eff = want & allowed;
    if eff == 0 {
        eff = DROPEFFECT_LINK.0 & allowed;
    }
    if eff == 0 {
        eff = DROPEFFECT_COPY.0 & allowed;
    }
    if eff == 0 {
        eff = DROPEFFECT_MOVE.0 & allowed;
    }
    if eff == 0 {
        // 最后兜底：源给什么效果都接下来（我们自己只建快捷方式 / 搬文件，不会误操作）
        eff = allowed & (DROPEFFECT_LINK.0 | DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0);
    }
    single_effect(eff, want)
}

// ---------- 建快捷方式 ----------

/// 文件名里不能用的字符（`\ / : * ? " < > |`）
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() {
        "快捷方式".to_string()
    } else {
        s
    }
}

/// 目标路径唯一化：已存在就加 " (2)"、" (3)"…
fn unique_lnk_path(dir: &Path, base: &str) -> PathBuf {
    let mut p = dir.join(format!("{base}.lnk"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{base} ({i}).lnk"));
        i += 1;
    }
    p
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 在 `dir` 里为虚拟 shell 项建一个 `.lnk`（PIDL 由调用方负责释放，这里只是借用）。
///
/// `SetIDList` 而不是 `SetPath` —— 虚拟项没有文件路径，只能按 PIDL 定位，
/// 这样建出来的快捷方式双击照样能打开「此电脑 / 回收站」。
unsafe fn create_shortcut_raw(
    dir: &Path,
    pidl: *const ITEMIDLIST,
    name: &str,
) -> Result<PathBuf, String> {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
        .map_err(|e| format!("创建快捷方式失败: {e}"))?;
    link.SetIDList(pidl)
        .map_err(|e| format!("设置快捷方式目标失败（{name}）: {e}"))?;
    let base = sanitize(name);
    let target = unique_lnk_path(dir, &base);
    let pf: IPersistFile = link.cast().map_err(|e| e.to_string())?;
    let w = to_wide(&target.to_string_lossy());
    pf.Save(PCWSTR(w.as_ptr()), true)
        .map_err(|e| format!("保存「{base}」失败: {e}"))?;
    Ok(target)
}

unsafe fn create_shortcut(dir: &Path, item: &NamedPidl) -> Result<PathBuf, String> {
    create_shortcut_raw(dir, item.pidl, &item.name)
}

// ---------- 落点兜底 ----------
//
// 实测（看 qidomin.log）：拖「此电脑 / 回收站」进来时只收到
//
//     DragEnter → DragOver → DragLeave        （前后 2 毫秒）
//
// **压根没有 Drop**。鼠标 2ms 内不可能移出窗口，是 OLE 自己把这次拖放掐了
// （透明 / 分层窗口的命中抖动，命中有时落到桌面层，OLE 便以为离开了目标）。
// 既然 DragEnter 已经把数据完整读到了，就先扣在手里：全局鼠标钩子见到左键抬起、
// 且光标确实停在这个分区窗口上，就当成一次落点自己处理掉。
// 正常路径（OLE 真的调了 Drop）下，Drop 会记下时间戳，兜底让位，不会重复建。

/// 一次"拖进来了、但 OLE 没给 Drop"的悬空拖放
struct PendingDrop {
    app: AppHandle,
    box_id: i64,
    /// HWND 不是 Send，存裸值（只用来做矩形命中测试，跨线程读矩形是安全的）
    hwnd: isize,
    /// 虚拟项：显示名 + PIDL 原始字节（PIDL 内部用相对偏移，整块复制即可还原）
    items: Vec<(String, Vec<u8>)>,
    /// 真实文件（有路径的那一路也一起兜底）
    files: Vec<PathBuf>,
    pos: (f64, f64),
    /// 扣留时刻（毫秒）：命中抖动反复 DragEnter 时，新鲜的扣留不必重做
    stashed_at: u64,
    /// 扣留时所在的拖动代次。下一轮拖动开始后这份记录立刻作废 ——
    /// 兜底是延时 400ms 才执行的，不认代次的话它会把上一轮的内容落到新一轮里。
    down_gen: u64,
}

static PENDING: std::sync::OnceLock<Mutex<Option<PendingDrop>>> = std::sync::OnceLock::new();
/// 最近一次"正经 Drop"的时间戳（毫秒）
static LAST_DROP_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pending() -> &'static Mutex<Option<PendingDrop>> {
    PENDING.get_or_init(|| Mutex::new(None))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 一次拖动只允许一个分区收下文件 —— 本模块最重要的一条不变量。
///
/// 为什么必须收口：**同一个物理落点会从三条通道分别送进来**
/// 1. 我们自己的 `IDropTarget::Drop`；
/// 2. WebView2 内建的拖放 → wry → tao 的 `WindowEvent::DragDrop::Drop`
///    （它注册在 WebView2 内部，`RevokeDragDrop` 够不着，所以关不掉）；
/// 3. OLE 没给 `Drop` 时，鼠标钩子的兜底（见 [`on_lbutton_up`]）。
///
/// 三条通道各自都会把文件搬一遍，于是"一次拖动、两个分区各收一份"。
/// 这里以「每次按下鼠标 = 新一次拖动」分代：同一次拖动里第一个落点受领，
/// 其余一律丢弃。按下事件来自全局鼠标钩子（见 [`on_lbutton_down`]）。
/// 拖动代次：每一次鼠标左键按下 +1。同一次拖动里三条通道共享同一个代次。
static DOWN_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 已经被哪个代次受领（`u64::MAX` = 还没有人受领）
static CLAIMED_GEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);
/// 最近一次受领的时刻（毫秒），兼作"钩子漏报按下"时的超时兜底
static CLAIM_AT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// 受领者（分区 id），只为了把日志说清楚
static CLAIMED_BY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

/// 鼠标左键按下 = 一次新拖动的起点：开一个新代次，受领名额随之重新开放。
///
/// 不这么做的话，一次拖动受领之后名额就一直是"已被占"，用户之后每一次拖放
/// 都会被当成重复投递而丢掉。
pub fn on_lbutton_down() {
    DOWN_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// 抢占"本次拖动的唯一落点"。只有第一个调用者能拿到 `true`。
///
/// 兜底：万一鼠标钩子漏报了按下（例如钩子是拖动中途才装上的），
/// 距上次受领超过 1.5 秒就允许重新受领 —— 宁可多收一次，也不能把整次拖放丢掉。
fn claim_drag() -> bool {
    use std::sync::atomic::Ordering;
    let gen = DOWN_GEN.load(Ordering::SeqCst);
    let now = now_ms();
    let stale = now.saturating_sub(CLAIM_AT_MS.load(Ordering::SeqCst)) >= 1500;
    let cur = CLAIMED_GEN.load(Ordering::SeqCst);
    if cur == gen && !stale {
        return false;
    }
    // 三条通道几乎同时到达，CAS 保证只有一个能把名额改成本代次
    if CLAIMED_GEN
        .compare_exchange(cur, gen, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    CLAIM_AT_MS.store(now, Ordering::SeqCst);
    true
}

/// 落点是从哪条通道来的（只为日志/排查）
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DropVia {
    /// 我们自己的 `IDropTarget`
    Ole,
    /// WebView2 内建拖放 → wry → tao 窗口事件
    Tao,
    /// 鼠标钩子的兜底
    Fallback,
}

impl DropVia {
    fn name(self) -> &'static str {
        match self {
            DropVia::Ole => "IDropTarget",
            DropVia::Tao => "tao事件",
            DropVia::Fallback => "兜底",
        }
    }
}

/// 分区窗口的标题（分区名），只为了让日志一眼能读懂
pub fn box_title(app: &AppHandle, box_id: i64) -> String {
    app.try_state::<crate::boxwin::BoxWindows>()
        .and_then(|b| b.label_for(box_id))
        .and_then(|l| app.get_webview_window(&l))
        .and_then(|w| w.title().ok())
        .unwrap_or_else(|| format!("#{box_id}"))
}

/// 落点准入：这次投递该不该由 `box_id` 收下。两条规则缺一不可。
///
/// 1. **只有光标下 z 序最上层的那个分区能收**。用户拖过来的文件是丢给"眼睛看到的
///    那一份"的；光标下若压着别的分区（重叠）、或者压着别的程序窗口，就谁也别接
///    —— 这正是"释放位置背后那个分区也把文件收下了"的根治办法。判定走
///    [`crate::boxwin::box_at_point`]（`WindowFromPoint` + 顶层祖先），与视觉一致。
/// 2. **一次拖动只放行一个分区**（见 [`claim_drag`]）。
fn admit(app: &AppHandle, box_id: i64, screen: (i32, i32), via: DropVia) -> bool {
    use std::sync::atomic::Ordering;
    let top = crate::boxwin::box_at_point(app, screen.0, screen.1, -1);
    if top != Some(box_id) {
        crate::log_msg(&format!(
            "shelldrop: 丢弃{}落点「{}」—— 光标({}, {})下不是它（{}）",
            via.name(),
            box_title(app, box_id),
            screen.0,
            screen.1,
            match top {
                Some(id) => format!("最上层是「{}」", box_title(app, id)),
                None => crate::boxwin::window_desc_at(screen.0, screen.1),
            }
        ));
        return false;
    }
    if !claim_drag() {
        crate::log_msg(&format!(
            "shelldrop: 丢弃{}落点「{}」—— 本次拖动已由「{}」受领",
            via.name(),
            box_title(app, box_id),
            box_title(app, CLAIMED_BY.load(Ordering::SeqCst))
        ));
        return false;
    }
    CLAIMED_BY.store(box_id, Ordering::SeqCst);
    // 落点已被接住：记下时间、收走兜底记录，免得同一批文件再落一次
    LAST_DROP_MS.store(now_ms(), Ordering::Relaxed);
    if let Ok(mut g) = pending().lock() {
        *g = None;
    }
    true
}

/// 所有"真实文件"落点的唯一入口：先过准入，再交给 [`crate::boxwin::run_drop`]。
pub fn route_drop(
    app: &AppHandle,
    box_id: i64,
    paths: Vec<String>,
    pos: (f64, f64),
    screen: (i32, i32),
    via: DropVia,
) {
    // 空载荷（例如页面内部的 HTML5 拖拽被报成一次 Drop）：什么也不用做，
    // 更不能占用本次拖动的受领名额
    if paths.is_empty() {
        return;
    }
    if !admit(app, box_id, screen, via) {
        return;
    }
    crate::log_msg(&format!(
        "shelldrop: 落点（{}）「{}」，{} 项",
        via.name(),
        box_title(app, box_id),
        paths.len()
    ));
    crate::boxwin::run_drop(app, box_id, paths, pos);
}

/// 同一条消息 800ms 内只打一次。
///
/// 透明窗口的"命中抖动"会让 OLE 连发 DragEnter/DragLeave（实测 2ms 一对），
/// 每次都原样打日志的话，一次拖动能刷出几百条同样的内容。
fn log_throttled(msg: &str) {
    static LAST: std::sync::Mutex<(String, u64)> = std::sync::Mutex::new((String::new(), 0));
    let now = now_ms();
    let pass = match LAST.lock() {
        Ok(mut g) => {
            let pass = g.0 != msg || now.saturating_sub(g.1) >= 800;
            if pass {
                *g = (msg.to_string(), now);
            }
            pass
        }
        Err(_) => true,
    };
    if pass {
        crate::log_msg(msg);
    }
}

/// 全局鼠标钩子见到左键抬起时调用：`(x, y)` 是屏幕坐标。
///
/// 光标停在悬空拖放记录的那个分区窗口上 → 等一小会儿（给 OLE 的 Drop 一次机会），
/// 没人接手就自己把落点办了。
pub fn on_lbutton_up(x: i32, y: i32) {
    let Some(mut p) = pending().lock().ok().and_then(|mut g| g.take()) else {
        return;
    };
    // 以**松手点**为准重新判定落点：记录里的窗口只是 DragEnter 那一刻命中的，
    // 拖动过程中光标完全可以挪到别的分区去（分区重叠时尤其常见）。
    match crate::boxwin::box_at_point(&p.app, x, y, -1) {
        Some(id) if id != p.box_id => {
            let hwnd = crate::boxwin::hwnd_for_box(&p.app, id).unwrap_or(p.hwnd);
            crate::log_msg(&format!(
                "shelldrop: 松手点在分区 {id}（记录的是 {}），改投到实际分区",
                p.box_id
            ));
            p.box_id = id;
            p.hwnd = hwnd;
        }
        Some(_) => {}
        None => {
            crate::log_msg(&format!(
                "shelldrop: 松手时不在任何分区上，丢弃悬空拖放（{} 项：{}）",
                p.items.len() + p.files.len(),
                p.files
                    .iter()
                    .map(|f| f.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            ));
            return;
        }
    }
    let release = (x, y);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        // OLE / tao 的 Drop 这会儿要是来了，就让它干活，兜底退场
        let last = LAST_DROP_MS.load(std::sync::atomic::Ordering::Relaxed);
        if now_ms().saturating_sub(last) < 1500 {
            crate::log_msg("shelldrop: 兜底让位（Drop 已由其它通道处理）");
            return;
        }
        apply_pending(p, release);
    });
}

fn apply_pending(p: PendingDrop, release: (i32, i32)) {
    // 陈旧兜底：扣留太久说明用户已经开始了新一轮拖动，这一份不能补落
    let age = now_ms().saturating_sub(p.stashed_at);
    if age > 2500 {
        crate::log_msg(&format!("shelldrop: 兜底记录已过期（{age}ms），丢弃"));
        return;
    }
    // 期间又按下过鼠标 = 用户开始了新一轮拖动，这一份已经过期
    if DOWN_GEN.load(std::sync::atomic::Ordering::SeqCst) != p.down_gen {
        crate::log_msg("shelldrop: 兜底记录来自上一次拖动，丢弃");
        return;
    }
    // 与另外两条通道共用准入闸门：一次拖动只允许一个分区收下，
    // 且必须是松手点下最上层的那个分区。
    if !admit(&p.app, p.box_id, release, DropVia::Fallback) {
        return;
    }
    crate::log_msg(&format!(
        "shelldrop: 兜底落点（OLE 没给 Drop）「{}」，{} 项",
        box_title(&p.app, p.box_id),
        p.items.len() + p.files.len()
    ));
    let (db, dir) = match p.app.try_state::<crate::db::Db>() {
        Some(d) => match crate::engine::box_dir_for(d.inner(), p.box_id) {
            Ok(dir) => (d.inner().clone(), dir),
            Err(e) => {
                crate::boxwin::emit_transfer_result(&p.app, p.box_id, Err(e));
                return;
            }
        },
        None => return,
    };
    let _ = db;
    let mut created = 0i64;
    let mut errors: Vec<String> = Vec::new();
    for (name, bytes) in &p.items {
        // 借用的 PIDL：不能 ILFree，用完就丢
        let pidl = bytes.as_ptr() as *const ITEMIDLIST;
        match unsafe { create_shortcut_raw(&dir, pidl, name) } {
            Ok(path) => {
                created += 1;
                crate::log_msg(&format!("shelldrop: 已建快捷方式「{}」", path.display()));
            }
            Err(e) => errors.push(e),
        }
    }
    if !p.files.is_empty() {
        let paths: Vec<String> = p
            .files
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect();
        crate::boxwin::run_drop(&p.app, p.box_id, paths, p.pos);
    }
    if created == 0 && p.files.is_empty() {
        return;
    }
    crate::commands::notify_box_files_changed(&p.app);
    crate::boxwin::emit_transfer_result(
        &p.app,
        p.box_id,
        Ok(crate::engine::TransferResult {
            copied: created,
            failed: errors.len() as i64,
            errors,
            ..Default::default()
        }),
    );
}

// ---------- IDropTarget ----------

#[implement(IDropTarget)]
struct BoxDropTarget {
    app: AppHandle,
    box_id: i64,
    label: String,
    hwnd: HWND,
    /// DragEnter 定下的效果，DragOver / Drop 沿用它
    effect: Cell<u32>,
    /// DragOver 日志的时间戳（毫秒）—— 一秒好几十次，必须节流
    last_log: Cell<u64>,
}

impl BoxDropTarget {
    /// 屏幕坐标 → 窗口内的**逻辑**坐标（前端 `box-item-*` 用的是逻辑像素）
    fn client_pos(&self, pt: &POINTL) -> (f64, f64) {
        self.client_pos_on(self.hwnd, &self.label, pt)
    }

    /// 同 [`Self::client_pos_on`]，但可指定"按哪个窗口换算"。
    ///
    /// 落点判定若纠正到了别的分区（OLE 抖动投错），坐标必须按**那个**分区的窗口
    /// 重新换算，否则前端拿到的插入位置会整体偏移一个窗口的距离。
    fn client_pos_on(&self, hwnd: HWND, label: &str, pt: &POINTL) -> (f64, f64) {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(hwnd, &mut p);
        }
        let scale = self
            .app
            .get_webview_window(label)
            .and_then(|w| w.scale_factor().ok())
            .unwrap_or(1.0)
            .max(0.01);
        (p.x as f64 / scale, p.y as f64 / scale)
    }

    fn emit_hover(&self, pt: &POINTL, leave: bool) {
        let (x, y) = if leave { (-1.0, -1.0) } else { self.client_pos(pt) };
        if let Some(w) = self.app.get_webview_window(&self.label) {
            let _ = w.emit(
                "box-item-hover",
                serde_json::json!({ "boxId": self.box_id, "x": x, "y": y }),
            );
        }
    }

    /// DragOver 日志（节流到 ~1 秒一条）：确认"源一直在问、且每次都给了非零效果"
    fn log_over(&self, eff: u32) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if now.saturating_sub(self.last_log.get()) < 1000 {
            return;
        }
        self.last_log.set(now);
        crate::log_msg(&format!("shelldrop: DragOver eff={eff:#x}"));
    }

    /// DragEnter 里把内容扣一份（OLE 不调 Drop 时的救命稻草）
    fn stash_pending(&self, pt: &POINTL, payload: &Payload) {
        // 命中抖动：同一窗口几百毫秒内反复 DragEnter。已有新鲜扣留就只更新位置，
        // 不重新解 PIDL / 拷路径（GetData + ILCombine 都不是免费的）
        if let Ok(g) = pending().lock() {
            if let Some(p) = g.as_ref() {
                if p.hwnd == self.hwnd.0 as isize
                    && now_ms().saturating_sub(p.stashed_at) < 400
                {
                    return;
                }
            }
        }
        let mut items: Vec<(String, Vec<u8>)> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        match payload {
            Payload::Files(v) => files = v.clone(),
            Payload::ShellItems(v) => {
                for it in v {
                    unsafe {
                        let sz = ILGetSize(Some(it.pidl)) as usize;
                        if sz == 0 {
                            continue;
                        }
                        let bytes =
                            std::slice::from_raw_parts(it.pidl as *const u8, sz).to_vec();
                        items.push((it.name.clone(), bytes));
                    }
                }
            }
            Payload::Unsupported => return,
        }
        if let Ok(mut g) = pending().lock() {
            *g = Some(PendingDrop {
                app: self.app.clone(),
                box_id: self.box_id,
                hwnd: self.hwnd.0 as isize,
                items,
                files,
                pos: self.client_pos(pt),
                stashed_at: now_ms(),
                down_gen: DOWN_GEN.load(std::sync::atomic::Ordering::SeqCst),
            });
        }
    }

    /// 诊断：这个屏幕点到底命中了谁（是不是本窗口）。
    ///
    /// 必须取**顶层祖先**再比：WebView2 的客户区被它自己的子窗口
    /// （`Chrome_WidgetWin_*` / `TAURI_DRAG_RESIZE_BORDERS`…）整个盖住，
    /// 直接拿 `WindowFromPoint` 的返回值对比，日志里会永远写着"命中其它窗口"，
    /// 把排查引向错误方向（实测过：那些"其它窗口"全都是本窗口的子窗口）。
    fn hit_debug(&self, pt: &POINTL) -> String {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
            let p = POINT { x: pt.x, y: pt.y };
            let hit = WindowFromPoint(p);
            let root = GetAncestor(hit, GA_ROOT);
            let root = if root.is_invalid() { hit } else { root };
            if root == self.hwnd {
                "命中本窗口".to_string()
            } else {
                format!("命中其它窗口 hwnd={:?}（本窗口 {:?}）", root, self.hwnd)
            }
        }
    }

    /// 虚拟项落进来：建快捷方式，并把结果告诉前端
    fn drop_shell_items(&self, items: Vec<NamedPidl>) {
        let db = match self.app.try_state::<crate::db::Db>() {
            Some(d) => d.inner().clone(),
            None => return,
        };
        let dir = match crate::engine::box_dir_for(&db, self.box_id) {
            Ok(d) => d,
            Err(e) => {
                crate::boxwin::emit_transfer_result(&self.app, self.box_id, Err(e));
                return;
            }
        };
        let mut created = 0i64;
        let mut errors: Vec<String> = Vec::new();
        for item in &items {
            match unsafe { create_shortcut(&dir, item) } {
                Ok(p) => {
                    created += 1;
                    crate::log_msg(&format!(
                        "shelldrop: 已在分区 {} 建快捷方式「{}」",
                        self.box_id,
                        p.display()
                    ));
                }
                Err(e) => {
                    crate::log_msg(&format!("shelldrop: {e}"));
                    errors.push(e);
                }
            }
        }
        // 分区目录变了（关联文件夹分区也能收到），广播一次让面板重读
        crate::commands::notify_box_files_changed(&self.app);
        crate::boxwin::emit_transfer_result(
            &self.app,
            self.box_id,
            Ok(crate::engine::TransferResult {
                copied: created,
                failed: errors.len() as i64,
                errors,
                ..Default::default()
            }),
        );
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for BoxDropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            let allowed = if pdweffect.is_null() {
                DROPEFFECT_COPY.0
            } else {
                (*pdweffect).0
            };
            let Some(data) = pdataobj.as_ref() else {
                self.effect.set(DROPEFFECT_NONE.0);
                if !pdweffect.is_null() {
                    *pdweffect = DROPEFFECT_NONE;
                }
                return Ok(());
            };
            let payload = read_payload(data);
            // 真实文件：对外报告 **COPY 优先**（single_effect 里 COPY 排在 MOVE 前面）。
            // 这是防丢文件的关键：透明窗口的命中抖动会让 OLE 半路发 DragLeave，
            // 若此时松手、Drop 没送过来，OLE 会把"最后一次报告的效果"还给拖动源 ——
            // 报了 MOVE 的话，资源管理器会按"目标已接收移动"的契约**把源文件删掉**
            // （实测：文件无端消失）。报 COPY 则源永远不动，真正的移动/复制
            // 由我们自己的落点处理按 Ctrl 状态决定（run_drop → transfer_into_box）。
            let want = match &payload {
                Payload::Files(_) => DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0,
                // 虚拟项只能"建一个快捷方式"，本质就是 LINK
                // （Explorer 拖它们时往往也只认 LINK，见 pick_effect）。
                Payload::ShellItems(_) => DROPEFFECT_LINK.0 | DROPEFFECT_COPY.0,
                Payload::Unsupported => 0,
            };
            let eff = pick_effect(want, allowed);
            log_throttled(&format!(
                "shelldrop: DragEnter allowed={allowed:#x} want={want:#x} → eff={eff:#x}，{}",
                self.hit_debug(pt)
            ));
            self.effect.set(eff);
            if !pdweffect.is_null() {
                *pdweffect = DROPEFFECT(eff);
            }
            if eff != 0 {
                // 先把内容扣下来：OLE 常常只给 DragEnter/DragOver 就 DragLeave，
                // 左键抬起时由兜底逻辑收尾（见 on_lbutton_up）
                self.stash_pending(pt, &payload);
                self.emit_hover(pt, false);
            }
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            let allowed = if pdweffect.is_null() {
                0
            } else {
                (*pdweffect).0
            };
            // 拖动过程中源给的"允许集合"是会变的（按了 Ctrl / Shift、拖过不同区域）。
            // 抱着 DragEnter 那一次的结果不放，一旦它落在新集合之外，
            // 源立刻把光标画成禁止，松手也不会调 Drop —— 所以每次都重挑一个。
            let eff = if allowed == 0 {
                self.effect.get()
            } else {
                pick_effect(self.effect.get(), allowed)
            };
            if eff != self.effect.get() {
                self.effect.set(eff);
                crate::log_msg(&format!(
                    "shelldrop: DragOver 效果改判 allowed={allowed:#x} → eff={eff:#x}"
                ));
            }
            if !pdweffect.is_null() {
                *pdweffect = DROPEFFECT(eff);
            }
            // 拖动中位置一直变，兜底用最后一次位置落点
            if let Ok(mut g) = pending().lock() {
                if let Some(p) = g.as_mut() {
                    if p.hwnd == self.hwnd.0 as isize {
                        p.pos = self.client_pos(pt);
                    }
                }
            }
            // DragOver 每动一下鼠标就来一次，必须节流，否则前端会被事件淹了
            if eff != 0 && crate::boxwin::should_emit_hover() {
                self.emit_hover(pt, false);
                self.log_over(eff);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        // 注意：这里**不清**兜底记录。命中抖动时 OLE 也会给 DragLeave，
        // 用户其实还按着鼠标停在窗口上；等左键抬起再决定（见 on_lbutton_up）。
        log_throttled("shelldrop: DragLeave（可能是真离开，也可能只是命中抖了一下）");
        self.emit_hover(&POINTL { x: 0, y: 0 }, true);
        self.effect.set(DROPEFFECT_NONE.0);
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        unsafe {
            let eff = self.effect.get();
            if !pdweffect.is_null() {
                *pdweffect = DROPEFFECT(eff);
            }
            // 先过准入。**注意顺序**：被拒时绝不能留下 LAST_DROP_MS 指纹，
            // 否则兜底通道会误以为"有人接住了"而让位，整次拖放就没人处理了。
            if !admit(&self.app, self.box_id, (pt.x, pt.y), DropVia::Ole) {
                return Ok(());
            }
            crate::log_msg(&format!(
                "shelldrop: Drop 被调用 eff={eff:#x}（已准入「{}」）",
                box_title(&self.app, self.box_id)
            ));
            let Some(data) = pdataobj.as_ref() else {
                crate::log_msg("shelldrop: Drop 拿不到数据对象");
                return Ok(());
            };
            let (x, y) = self.client_pos(pt);
            match read_payload(data) {
                Payload::Files(paths) => {
                    let paths: Vec<String> = paths
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    crate::log_msg(&format!(
                        "shelldrop: 落点（IDropTarget）分区 {}，{} 项",
                        self.box_id,
                        paths.len()
                    ));
                    crate::boxwin::run_drop(&self.app, self.box_id, paths, (x, y));
                }
                Payload::ShellItems(items) => {
                    crate::log_msg(&format!("shelldrop: 落点 {} 个虚拟项", items.len()));
                    self.drop_shell_items(items);
                }
                Payload::Unsupported => {}
            }
        }
        Ok(())
    }
}

/// 主线程（窗口所属、带消息泵的那个线程）的 id。
///
/// `RegisterDragDrop` **必须在跑消息泵的那个线程上调用**：OLE 拖放是"发消息给
/// 注册线程"的方式回调 `IDropTarget` 的，注册在别的线程（比如 Tauri 异步命令用的
/// tokio 工作线程）上，DragEnter / Drop 就永远不会进来 —— 表现是"系统装好了、
/// 拖过去光标直接禁止"。所以记住主线程，注册前先对齐线程。
static MAIN_THREAD: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

/// 在程序启动时调用一次，记下主线程
pub fn mark_main_thread() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

fn on_main_thread() -> bool {
    MAIN_THREAD
        .get()
        .map(|id| *id == std::thread::current().id())
        .unwrap_or(true)
}

/// 接管分区窗口的落点（替换 tao 内置那个只认 `CF_HDROP` 的）。
///
/// 必须在窗口**创建之后**调用 —— 先于 tao 注册的话，它那句
/// `assert!(RegisterDragDrop(...).is_ok())` 会因"已注册"失败而 panic。
pub fn install(app: &AppHandle, win: &WebviewWindow, box_id: i64) {
    let label = win.label().to_string();
    if !on_main_thread() {
        // 切回窗口所属线程再注册（异步命令建窗时会走到这里）
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = app2.get_webview_window(&label) {
                register_now(&app2, &w, box_id);
            }
        });
        return;
    }
    register_now(app, win, box_id);
}

fn register_now(app: &AppHandle, win: &WebviewWindow, box_id: i64) {
    let Ok(raw) = win.hwnd() else {
        return;
    };
    let hwnd = HWND(raw.0 as *mut core::ffi::c_void);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let target: IDropTarget = BoxDropTarget {
            app: app.clone(),
            box_id,
            label: win.label().to_string(),
            hwnd,
            effect: Cell::new(DROPEFFECT_NONE.0),
            last_log: Cell::new(0),
        }
        .into();
        // 顶层 + 整棵子窗口树一起接管（子窗口是 WebView2 的地盘，见下）
        register_on_tree(hwnd, &target, 4);
        crate::log_msg(&format!("shelldrop: 分区 {box_id} 已接管拖放落点（含子窗口）"));
    }
}

/// 把 `hwnd` **及其所有子孙窗口**都注册成同一个落点。
///
/// 分区窗口的客户区被 WebView2 的子窗口（`Chrome_WidgetWin_0` /
/// `Chrome_RenderWidgetHostHWND`）整个盖住：OLE 做命中测试时一落到子窗口上，
/// 就认为离开了顶层那个落点 —— 实测表现是 DragEnter 后 2ms 立刻 DragLeave、
/// 永远等不来 Drop（光标还是禁止）。而且子窗口上 Chromium 自己注册过落点，
/// 它只服务于网页拖放、不认 shell 项。所以必须连子窗口一起换成我们的。
unsafe fn register_on_tree(hwnd: HWND, target: &IDropTarget, depth: usize) {
    // 换掉原有的（tao 顶层注册的、Chromium 子窗口注册的都算）；没有就忽略
    let _ = RevokeDragDrop(hwnd);
    if let Err(e) = RegisterDragDrop(hwnd, target) {
        crate::log_msg(&format!("shelldrop: hwnd={:?} 接管落点失败：{e}", hwnd.0));
    }
    if depth == 0 {
        return;
    }
    // 枚举直接子窗口，递归往下注册
    let mut after = None;
    loop {
        match FindWindowExW(Some(hwnd), after, None, None) {
            Ok(child) => {
                register_on_tree(child, target, depth - 1);
                after = Some(child);
            }
            Err(_) => break,
        }
    }
}

/// 窗口销毁前解绑，免得系统还拿着一个指向已释放对象的落点
pub fn uninstall(win: &WebviewWindow) {
    if let Ok(raw) = win.hwnd() {
        let hwnd = HWND(raw.0 as *mut core::ffi::c_void);
        unsafe {
            revoke_on_tree(hwnd, 4);
        }
    }
}

/// `register_on_tree` 的反操作：整棵树逐个解绑
unsafe fn revoke_on_tree(hwnd: HWND, depth: usize) {
    let _ = RevokeDragDrop(hwnd);
    if depth == 0 {
        return;
    }
    let mut after = None;
    loop {
        match FindWindowExW(Some(hwnd), after, None, None) {
            Ok(child) => {
                revoke_on_tree(child, depth - 1);
                after = Some(child);
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        FOLDERID_ComputerFolder, FOLDERID_Desktop, FOLDERID_RecycleBinFolder, ILGetSize,
        SHGetKnownFolderIDList,
    };

    /// shell 调用前的 COM 初始化（COM 是线程相关的，每个用例都要自己来一遍）
    fn init_com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
    }

    /// 回归：拖进来的 `CFSTR_SHELLIDLIST` 能正确解出每一项的完整 PIDL。
    ///
    /// 手工拼一块 CIDA（父 = 桌面，子 = 此电脑），解出来的项显示名必须正确。
    #[test]
    fn parse_cida_joins_parent_and_child() {
        init_com();
        unsafe {
            let parent = SHGetKnownFolderIDList(&FOLDERID_Desktop, 0, None).expect("桌面 PIDL");
            let child =
                SHGetKnownFolderIDList(&FOLDERID_ComputerFolder, 0, None).expect("此电脑 PIDL");
            let ps = ILGetSize(Some(parent)) as usize;
            let cs = ILGetSize(Some(child)) as usize;

            // [cidl][aoffset0][aoffset1][父 PIDL 字节][子 PIDL 字节]
            let mut buf = vec![0u8; 12 + ps + cs];
            buf[0..4].copy_from_slice(&1u32.to_ne_bytes());
            buf[4..8].copy_from_slice(&12u32.to_ne_bytes());
            buf[8..12].copy_from_slice(&((12 + ps) as u32).to_ne_bytes());
            buf[12..12 + ps].copy_from_slice(std::slice::from_raw_parts(parent.cast::<u8>(), ps));
            buf[12 + ps..].copy_from_slice(std::slice::from_raw_parts(child.cast::<u8>(), cs));

            let items = parse_cida(buf.as_ptr() as *const core::ffi::c_void);
            assert_eq!(items.len(), 1, "应当解出 1 项");
            assert_eq!(pidl_name(items[0]), "此电脑");

            for p in items {
                ILFree(Some(p));
            }
            ILFree(Some(parent));
            ILFree(Some(child));
        }
    }

    /// 回归：源只认 LINK（拖虚拟项时 Explorer 常这样）也要能接下来。
    ///
    /// 以前这里返回 0 → 光标变禁止 → 此电脑 / 回收站永远拖不进分区。
    #[test]
    fn pick_effect_falls_back_to_any_allowed() {
        // 想要 COPY、源只给 LINK → 接 LINK（建快捷方式本来就是 LINK 语义）
        assert_eq!(
            pick_effect(DROPEFFECT_COPY.0, DROPEFFECT_LINK.0),
            DROPEFFECT_LINK.0
        );
        // 想要 LINK|COPY、源只给 MOVE → 也接（我们不会真移动虚拟项）
        assert_eq!(
            pick_effect(DROPEFFECT_LINK.0 | DROPEFFECT_COPY.0, DROPEFFECT_MOVE.0),
            DROPEFFECT_MOVE.0
        );
        // 认不出的内容（want = 0）一律不接
        assert_eq!(pick_effect(0, DROPEFFECT_COPY.0), 0);
        // 正常情况：交集非空就直接用
        assert_eq!(
            pick_effect(DROPEFFECT_MOVE.0, DROPEFFECT_MOVE.0 | DROPEFFECT_COPY.0),
            DROPEFFECT_MOVE.0
        );
    }

    /// 回归：此电脑 / 回收站这类**没有文件路径**的桌面图标也能落进分区 ——
    /// 在分区目录里生成指向它们的 `.lnk`，名字与图标都保持系统原样。
    #[test]
    fn shortcut_for_virtual_items() {
        init_com();
        let dir = std::env::temp_dir().join("qd_shelldrop_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");

        for (id, expect) in [
            (FOLDERID_ComputerFolder, "此电脑"),
            (FOLDERID_RecycleBinFolder, "回收站"),
        ] {
            unsafe {
                let pidl = SHGetKnownFolderIDList(&id, 0, None).expect("已知文件夹 PIDL");
                let item = NamedPidl {
                    pidl,
                    name: pidl_name(pidl),
                };
                assert_eq!(item.name, expect);

                let out = create_shortcut(&dir, &item).expect("建快捷方式");
                assert!(out.exists(), "快捷方式应当已生成：{}", out.display());
                assert!(
                    out.extension().map(|e| e == "lnk").unwrap_or(false),
                    "应当是 .lnk：{}",
                    out.display()
                );

                // 图标也要正常：快捷方式取不到图标的话，分区里看到的就是一个白块
                let png = crate::icons::icon_png(&out.to_string_lossy(), 96);
                assert!(!png.is_empty(), "「{expect}」的快捷方式取不到图标");
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

}
