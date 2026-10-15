//! 鼠标快捷键：全局低级钩子（WH_MOUSE_LL / WH_KEYBOARD_LL）+ SendInput 模拟按键。
//!
//! 只用 Windows 标准鼠标键（中键 / 双击中键 / 侧键 X1 / X2），任何带侧键的鼠标、
//! 零驱动、零品牌依赖。鼠标上那些固件内部动作（罗技 G6~G9 之类）Windows 收不到事件，
//! 本模块管不了、也不去管。
//!
//! - 总开关关闭（默认）：钩子虽然挂载但一律放行，对系统零影响；
//! - 「识别信号」模式：设置页点「点击识别」后 6 秒内按下的中键/侧键，
//!   通过 `mouse-captured` 事件回传前端填进映射表（按键本身被吞掉，不产生副作用）；
//! - 命中绑定的按键：执行动作并吞掉（其他程序收不到）；
//!   系统操作（全选/复制/粘贴…）用 SendInput 注入对应键盘组合键到当前焦点窗口。
//!
//! ## 空格组合（`mode == "spaceAfter"`）：两个键同时按着才算
//!
//! 绑定表达的是「**鼠标键 + 空格**」这一对，谁先按都算。**触发条件只有一条 ——
//! 后按下的那一下落下时，另一个键还按着**（等价于「两个键同时按着就触发」）：
//!
//! | 顺序 | 处理 |
//! | --- | --- |
//! | 先按住鼠标键 → 点空格 | 鼠标键**完整放行**给当前软件（3D 旋转视图等照常）；按着的时候每点一下空格都触发 |
//! | 先按住空格 → 点鼠标键 | 鼠标按下那一刻空格按着 → 立刻触发，**鼠标键被吞掉**；按住空格时可以一直点鼠标键 |
//!
//! 只按着一个、另一个单独点一下（两次按下在时间上不重叠）→ **不算**；两个键都松开就作废。
//!
//! 「按着」用的是**我们自己记的物理按键状态**（`SPACE_DOWN` / `MOUSE_DOWN`：钩子里逐条
//! 物理事件更新），不是 `GetAsyncKeyState`。原因：组合成立后这一下空格要整个吞掉（连抬起
//! 一起吞），被吞掉的抬起**不会**被系统记进键盘/鼠标状态表 —— 照 `GetAsyncKeyState` 读，
//! 空格就"永远按着"，于是之后单个中键也被判成组合。自己记就不会被"吞事件"带偏。
//!
//! 空格先按住时必须先"扣下"再决定：普通敲空格绝不能被 hook 吃掉，所以第一次空格按下
//! 先吞住不发给系统，若直到松开都没等到鼠标键，就把这次空格**原样补发**出去（抬起时补，
//! 或期间按了别的键时立刻补，保证按键顺序不乱）。组合一旦成立，这次空格就整个作废
//! （自动重复与抬起一并吞掉），当前程序一个空格都收不到。
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::CallNextHookEx;

use crate::db::Db;

/// 一条鼠标快捷键映射（与前端共享，camelCase）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MouseBinding {
    pub id: u64,
    /// middle | middle2 | x1 | x2
    pub button: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// 空格作为修饰键（传统写法：先按住空格再按键）。`mode == "spaceAfter"` 时不看这个字段
    #[serde(default)]
    pub space: bool,
    /// 触发顺序："" / "press" = 按下按键即触发；
    /// "spaceAfter" = 「按键 + 空格」组合，**不分先后**，两个键同时按着就触发（详见模块头注释）
    #[serde(default)]
    pub mode: String,
    /// 动作 key，见 run_action
    pub action: String,
    pub enabled: bool,
}

/// 「鼠标键 + 空格」组合的模式名（沿用旧值，老设置不用迁移）
const CHORD_MODE: &str = "spaceAfter";
const VK_SPACE_CODE: u32 = 0x20;

static ENABLED: AtomicBool = AtomicBool::new(false);
static BINDINGS: RwLock<Vec<MouseBinding>> = RwLock::new(Vec::new());
/// 「识别信号」截止时间（Unix 毫秒），0 = 未在识别
static CAPTURE_UNTIL: AtomicU64 = AtomicU64::new(0);
/// 需要吞掉的下次抬起事件：0 无 / 1 中键 / 2 侧键1 / 3 侧键2
static PENDING_UP: AtomicU32 = AtomicU32::new(0);
/// 空格的**物理**按住状态（在 kb_proc 里每条空格事件更新，包括被我们吞掉的那些）。
/// 组合的唯一触发判据靠它 —— 不能用 GetAsyncKeyState，理由见模块头注释
static SPACE_DOWN: AtomicBool = AtomicBool::new(false);
/// 鼠标键的**物理**按住状态位图：bit0 中键 / bit1 侧键1 / bit2 侧键2（在 mouse_proc 里更新）
static MOUSE_DOWN: AtomicU32 = AtomicU32::new(0);
/// 空格组合是否启用（总开关开着 + 表里确实有这类绑定）。为 false 时一个空格都不碰
static HAS_CHORD: AtomicBool = AtomicBool::new(false);

// ---------- 双击中键（button == "middle2"）----------
/// 上一次中键按下的时间（毫秒），用于判双击
static MID_LAST_DOWN_MS: AtomicU64 = AtomicU64::new(0);
/// 最近一次双击成立的时间（毫秒）：待发的"单击"见到它就让自己作废
static MID_LAST_DOUBLE_MS: AtomicU64 = AtomicU64::new(0);
/// 表里有没有启用中的「双击中键」绑定。为 false 时中键行为与从前完全一致
static HAS_MID_DOUBLE: AtomicBool = AtomicBool::new(false);
/// 识别模式里上一次中键按下的时间（判双击用），0 = 无
static CAP_MID_MS: AtomicU64 = AtomicU64::new(0);
/// 双击期间被扣住的"单击"动作：(按下时间戳, 绑定)。450ms 内没等到双击就照常触发；
/// 带时间戳是为了配对 —— 连续快速按下时，旧线程不许把新扣的单击抢走执行
static PENDING_SINGLE: std::sync::Mutex<Option<(u64, MouseBinding)>> = std::sync::Mutex::new(None);
/// 空格被"扣下"：true = 这一下空格已经吞掉，还没决定放不放行
static SPACE_ARMED: AtomicBool = AtomicBool::new(false);
/// 已经用 SendInput 补发过空格的**按下**（后面的真实抬起要放行，与它配对）
static SPACE_REPLAYED: AtomicBool = AtomicBool::new(false);
/// 组合成立：这一下空格整个作废（自动重复与抬起都吞掉，当前程序收不到空格）
static SPACE_MUTED: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;

/// 保存设置时调用：热更新生效的绑定表（无需重启）
pub fn update_config(enabled: bool, bindings: Vec<MouseBinding>) {
    ENABLED.store(enabled, Ordering::SeqCst);
    HAS_CHORD.store(enabled && has_chord_binding(&bindings), Ordering::SeqCst);
    HAS_MID_DOUBLE.store(
        enabled && bindings.iter().any(|x| x.enabled && x.button == "middle2"),
        Ordering::SeqCst,
    );
    if let Ok(mut b) = BINDINGS.write() {
        *b = bindings;
    }
}

/// 进入「按一下鼠标识别信号」模式（6 秒超时；Esc 可取消）
pub fn begin_capture() {
    CAPTURE_UNTIL.store(now_ms() + 6000, Ordering::SeqCst);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn capture_active() -> bool {
    let until = CAPTURE_UNTIL.load(Ordering::SeqCst);
    until > 0 && now_ms() < until
}

fn end_capture() {
    CAPTURE_UNTIL.store(0, Ordering::SeqCst);
}

/// 当前按住的修饰键（Ctrl, Shift, Alt, 空格）
fn mods_down() -> (bool, bool, bool, bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT, VIRTUAL_KEY,
    };
    unsafe {
        let down = |vk: VIRTUAL_KEY| (GetAsyncKeyState(vk.0 as i32) as u16) & 0x8000 != 0;
        (
            down(VK_CONTROL),
            down(VK_SHIFT),
            down(VK_MENU),
            // 空格读我们自己记的物理状态：组合成立时这一下空格被整个吞掉，
            // 系统的键盘状态表会停在"按着"，照着读就会把单个鼠标键也当成组合
            SPACE_DOWN.load(Ordering::SeqCst),
        )
    }
}

/// 精确匹配（修饰键必须完全一致）：已启用的绑定里找第一条
fn match_binding(
    button: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    space: bool,
) -> Option<MouseBinding> {
    let b = BINDINGS.read().unwrap_or_else(|e| e.into_inner());
    b.iter()
        .find(|x| {
            x.enabled
                && x.button == button
                && x.ctrl == ctrl
                && x.shift == shift
                && x.alt == alt
                && x.space == space
        })
        .cloned()
}

fn is_mouse_button(button: &str) -> bool {
    matches!(button, "middle" | "x1" | "x2")
}

/// 表里有没有启用中的「鼠标键 + 空格」组合
fn has_chord_binding(list: &[MouseBinding]) -> bool {
    list.iter()
        .any(|x| x.enabled && x.mode == CHORD_MODE && is_mouse_button(&x.button))
}

/// 找「鼠标键 + 空格」组合绑定。
///
/// 和 [`match_binding`] 的区别：**不看 `space` 字段**。空格先按住时，`mods_down()`
/// 会报 space=true，精确匹配就会落空 —— 可组合本来就该无视这一点。
fn match_chord_binding(
    button: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<MouseBinding> {
    let b = BINDINGS.read().unwrap_or_else(|e| e.into_inner());
    b.iter()
        .find(|x| {
            x.enabled
                && x.button == button
                && x.ctrl == ctrl
                && x.shift == shift
                && x.alt == alt
                && x.mode == CHORD_MODE
        })
        .cloned()
}

/// 记一条鼠标键的物理状态（在 mouse_proc 里每条按下 / 抬起都调一次，
/// 放在任何"吞掉这次事件"的判断之前 —— 吞事件不影响"手确实按着"这个事实）
fn set_mouse_phys(up_code: u32, down: bool) {
    let bit = 1u32 << (up_code - 1);
    if down {
        MOUSE_DOWN.fetch_or(bit, Ordering::SeqCst);
    } else {
        MOUSE_DOWN.fetch_and(!bit, Ordering::SeqCst);
    }
}

/// 某个鼠标键此刻是否物理按着（up_code：1 中键 / 2 侧键1 / 3 侧键2）
fn mouse_button_held(up_code: u32) -> bool {
    up_code != 0 && MOUSE_DOWN.load(Ordering::SeqCst) & (1u32 << (up_code - 1)) != 0
}

/// 这个鼠标键有没有「空格组合」绑定（只看是不是这类绑定，不管修饰键）
fn is_chord_button(name: &str) -> bool {
    let b = BINDINGS.read().unwrap_or_else(|e| e.into_inner());
    b.iter()
        .any(|x| x.enabled && x.mode == CHORD_MODE && x.button == name)
}

/// 此刻有没有「空格组合」绑定的鼠标键正按着 —— 空格这边触发靠的就是它。
/// 返回 up_code（0 = 没有）：1 中键 / 2 侧键1 / 3 侧键2
fn chord_button_held() -> u32 {
    if !HAS_CHORD.load(Ordering::SeqCst) {
        return 0;
    }
    for (up_code, name) in [(1u32, "middle"), (2, "x1"), (3, "x2")] {
        if mouse_button_held(up_code) && is_chord_button(name) {
            return up_code;
        }
    }
    0
}

/// 组合键成立 → 这一下空格整个作废。
///
/// 已经补发过按下的话不能吞它的抬起（否则当前程序那边留一个按下的空格不放开），
/// 只把"还扣着的那一下"作废。
fn mute_space() {
    let replayed = SPACE_REPLAYED.swap(false, Ordering::SeqCst);
    SPACE_ARMED.store(false, Ordering::SeqCst);
    SPACE_MUTED.store(!replayed, Ordering::SeqCst);
}

fn emit_json(app: &AppHandle, event: &str, payload: serde_json::Value) {
    let _ = app.emit(event, payload);
}

/// 执行动作：系统组合键走 SendInput 注入到当前焦点窗口；Qidomin 功能走后台线程
fn run_action(action: &str) {
    // (修饰键 VK, 主键 VK)：0x11=Ctrl 0x41~0x5A=A~Z 0x2E=Del 0x74=F5
    let (mods, key): (&[u32], u32) = match action {
        "selectAll" => (&[0x11], 0x41),
        "copy" => (&[0x11], 0x43),
        "cut" => (&[0x11], 0x58),
        "paste" => (&[0x11], 0x56),
        "undo" => (&[0x11], 0x5A),
        "redo" => (&[0x11], 0x59),
        "save" => (&[0x11], 0x53),
        "delete" => (&[], 0x2E),
        "refresh" => (&[], 0x74),
        _ => {
            run_app_action(action);
            return;
        }
    };
    send_combo(mods, key);
}

/// 注入键盘事件：`(VK, 是否抬起)` 序列。
///
/// 注入的事件同样会经过我们自己的低级钩子，所以钩子里必须按 `LLKHF_INJECTED`
/// 直接放行，否则自己发的空格会被自己再吞一次。
fn send_keys(seq: &[(u32, bool)]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_KEYBOARD, INPUT_0, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY,
    };
    let mut inputs: Vec<INPUT> = Vec::with_capacity(seq.len());
    for &(vk, up) in seq {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk as u16),
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
        });
    }
    if !inputs.is_empty() {
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// 注入一组键盘事件：修饰键按下 → 主键按下抬起 → 修饰键抬起
fn send_combo(mods: &[u32], key: u32) {
    let mut seq: Vec<(u32, bool)> = Vec::with_capacity(mods.len() * 2 + 2);
    for &m in mods {
        seq.push((m, false));
    }
    seq.push((key, false));
    seq.push((key, true));
    for &m in mods.iter().rev() {
        seq.push((m, true));
    }
    send_keys(&seq);
}

/// Qidomin 自身的功能动作：窗口/分区操作回主线程，重活（整理）放后台线程
fn run_app_action(action: &str) {
    let Some(app) = APP.get() else { return };
    let app = app.clone();
    let action = action.to_string();
    std::thread::spawn(move || match action.as_str() {
        "toggleBoxes" => {
            let a2 = app.clone();
            let _ = app.run_on_main_thread(move || crate::boxwin::toggle_all_boxes(&a2));
        }
        "toggleSearch" => {
            // 取当前光标位置（物理像素），面板左下角对齐它弹出
            let mut pt = windows::Win32::Foundation::POINT::default();
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
            }
            let pos = (pt.x, pt.y);
            let a2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                crate::show_search(&a2, crate::SearchAnchor::Point(pos.0, pos.1))
            });
        }
        "newBox" => {
            let a2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                let db = a2.state::<Db>();
                let name = crate::next_box_name(&db);
                let _ = crate::create_box_and_open(&a2, &db, &name, "#15b6c9");
            });
        }
        "organize" => {
            // 与托盘"一键整理"一致：带出主窗口 + 发预览请求，确认后才真正搬
            let a2 = app.clone();
            let _ = app.run_on_main_thread(move || {
                crate::show_main(&a2);
                let _ = tauri::Emitter::emit(&a2, "organize-request", ());
            });
        }
        "toggleNotes" => {
            let db = app.state::<Db>();
            let on = !crate::notes::is_enabled(&db);
            let _ = crate::notes::set_enabled(&app, &db, on);
        }
        _ => {}
    });
}

/// 鼠标低级钩子：中键 / 侧键 X1 X2 的识别与绑定执行
unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        use windows::Win32::UI::WindowsAndMessaging::MSLLHOOKSTRUCT;
        let msg = wp.0 as u32;
        let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
        // 左键按下 = 一次新拖动（或一次普通点击）的起点：重新开放落点受领。
        // 不重置的话，上一次拖动的"已受领"标记会把之后每一次拖放都否掉
        // （见 shelldrop::admit 的"一次拖动只允许一个分区收下"）。
        if msg == WM_LBUTTONDOWN {
            crate::shelldrop::on_lbutton_down();
        }
        // 左键抬起：给"OLE 没给 Drop"的悬空拖放一个收尾机会
        // （此电脑 / 回收站拖进分区时，OLE 常常只发 DragEnter 就掐了拖放，
        //   详见 shelldrop.rs 的兜底注释）。这里永远放行，不影响正常点击。
        if msg == WM_LBUTTONUP {
            crate::shelldrop::on_lbutton_up(info.pt.x, info.pt.y);
        }
        let is_down = msg == WM_MBUTTONDOWN || msg == WM_XBUTTONDOWN;
        let is_up = msg == WM_MBUTTONUP || msg == WM_XBUTTONUP;
        if is_down || is_up {
            let button = if msg == WM_MBUTTONDOWN || msg == WM_MBUTTONUP {
                "middle"
            } else {
                // XBUTTON 高位是按键编号：1=X1（前进）、2=X2（后退）；
                // 更多侧键的鼠标，驱动一般也映射成 X1/X2 上报
                match (info.mouseData >> 16) & 0xFFFF {
                    1 => "x1",
                    2 => "x2",
                    _ => "",
                }
            };
            let up_code = match button {
                "middle" => 1,
                "x1" => 2,
                "x2" => 3,
                _ => 0,
            };
            // 先把物理状态记下来：不管这次事件等下吞不吞，"手按着"这个事实都要准确 ——
            // 「两个键同时按着」这个唯一触发条件全靠它
            if up_code != 0 {
                set_mouse_phys(up_code, is_down);
            }
            if is_up {
                // 与被吞掉的按下配对的抬起：一并吞掉，避免"按下没生效、抬起却点到了"
                if up_code != 0 && PENDING_UP.swap(0, Ordering::SeqCst) == up_code {
                    return LRESULT(1);
                }
            } else if !button.is_empty() {
                // 识别模式：把按下的键回传设置页（左/右键不识别、照常放行，避免误绑定锁死鼠标）
                if capture_active() {
                    if button == "middle" {
                        // 中键：连按两下算「双击中键」。第一下先扣住，
                        // 550ms 内没等到第二下就按普通「中键」上报。
                        let now = now_ms();
                        let last = CAP_MID_MS.swap(now, Ordering::SeqCst);
                        PENDING_UP.store(up_code, Ordering::SeqCst);
                        let until = CAPTURE_UNTIL.load(Ordering::SeqCst);
                        if last > 0 && now.saturating_sub(last) < 500 {
                            // 双击成立：直接上报 middle2
                            CAP_MID_MS.store(0, Ordering::SeqCst);
                            end_capture();
                            if let Some(app) = APP.get() {
                                let (c, s, a, sp) = mods_down();
                                emit_json(
                                    app,
                                    "mouse-captured",
                                    serde_json::json!({
                                        "button": "middle2",
                                        "ctrl": c, "shift": s, "alt": a, "space": sp
                                    }),
                                );
                            }
                        } else {
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(560));
                                // 还在同一个识别会话、且期间没有第二下 → 按「中键」上报
                                if CAPTURE_UNTIL.load(Ordering::SeqCst) == until
                                    && CAP_MID_MS.load(Ordering::SeqCst) == now
                                {
                                    CAP_MID_MS.store(0, Ordering::SeqCst);
                                    end_capture();
                                    if let Some(app) = APP.get() {
                                        let (c, s, a, sp) = mods_down();
                                        emit_json(
                                            app,
                                            "mouse-captured",
                                            serde_json::json!({
                                                "button": "middle",
                                                "ctrl": c, "shift": s, "alt": a, "space": sp
                                            }),
                                        );
                                    }
                                }
                            });
                        }
                        return LRESULT(1);
                    }
                    end_capture();
                    PENDING_UP.store(up_code, Ordering::SeqCst);
                    if let Some(app) = APP.get() {
                        let (c, s, a, sp) = mods_down();
                        emit_json(
                            app,
                            "mouse-captured",
                            serde_json::json!({
                                "button": button, "ctrl": c, "shift": s, "alt": a, "space": sp
                            }),
                        );
                    }
                    return LRESULT(1);
                }
                // 双击中键检测：只在有启用中的 middle2 绑定时才介入，
                // 其余情况中键行为与从前完全一致。
                // **空格按着时不参与**：那一下中键是「鼠标键 + 空格」组合的第二下，
                // 不能当双击的第一下（否则组合里连点两下中键会误触发「双击中键」）；
                // 顺手把上一次按下的时间戳清掉，免得组合松手之后被当成双击的第一下。
                if button == "middle" && HAS_MID_DOUBLE.load(Ordering::SeqCst) {
                    if SPACE_DOWN.load(Ordering::SeqCst) {
                        MID_LAST_DOWN_MS.store(0, Ordering::SeqCst);
                    } else {
                        let now = now_ms();
                        let last = MID_LAST_DOWN_MS.swap(now, Ordering::SeqCst);
                        if last > 0 && now.saturating_sub(last) < 500 {
                            // 双击成立：作废待发的单击，尝试 middle2 绑定
                            MID_LAST_DOUBLE_MS.store(now, Ordering::SeqCst);
                            if let Ok(mut g) = PENDING_SINGLE.lock() {
                                *g = None;
                            }
                            let (c, s, a, sp) = mods_down();
                            if let Some(b) = match_binding("middle2", c, s, a, sp) {
                                PENDING_UP.store(1, Ordering::SeqCst);
                                run_action(&b.action);
                                return LRESULT(1);
                            }
                            // 没匹配上 middle2 → 这次按下按普通中键继续走
                        }
                    }
                }
                // 命中启用的绑定：执行动作并吞掉整个按键
                if ENABLED.load(Ordering::SeqCst) {
                    let (c, s, a, sp) = mods_down();
                    // ① 空格按着 → 这次鼠标按下就是组合的第二下（两个键同时按着）→ 立刻触发。
                    //    显式把空格当修饰键的老式绑定（space=true）优先，其次是不分先后的组合。
                    if sp {
                        if let Some(b) =
                            match_binding(button, c, s, a, true).or_else(|| match_chord_binding(button, c, s, a))
                        {
                            // 空格先按住时它是"扣着"的（没发给系统），这里连同鼠标键一起吞掉
                            PENDING_UP.store(up_code, Ordering::SeqCst);
                            mute_space();
                            run_action(&b.action);
                            return LRESULT(1);
                        }
                    }
                    // ② 常规精确匹配
                    if let Some(b) = match_binding(button, c, s, a, sp) {
                        if b.mode == CHORD_MODE {
                            // 先按鼠标键：完整放行给当前软件（3D 旋转视图等照常）。
                            // 这里什么都不用记 —— 按着的时候点空格，空格那边会回来问
                            // "有没有组合键按着"（chord_button_held），靠的是物理状态。
                            // 不吞、不触发，走放行。**单独点这一下不触发**（空格没按着）。
                        } else {
                            // 「双击中键」启用时，单击绑定延迟 450ms 再触发：
                            // 期间若双击成立，这条单击自动作废（否则双击必先触发单击）
                            if button == "middle" && HAS_MID_DOUBLE.load(Ordering::SeqCst) {
                                let press_ms = now_ms();
                                if let Ok(mut g) = PENDING_SINGLE.lock() {
                                    *g = Some((press_ms, b.clone()));
                                }
                                PENDING_UP.store(up_code, Ordering::SeqCst);
                                std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_millis(450));
                                    let cancelled =
                                        MID_LAST_DOUBLE_MS.load(Ordering::SeqCst) > press_ms;
                                    // 只执行"自己这一次按下"扣下的单击：别的线程扣的不归我管
                                    let mine = PENDING_SINGLE
                                        .lock()
                                        .ok()
                                        .and_then(|mut g| match *g {
                                            Some((t, _)) if t == press_ms => {
                                                g.take().map(|(_, b)| b)
                                            }
                                            _ => None,
                                        });
                                    if !cancelled && mine.is_some() {
                                        run_action(&b.action);
                                    }
                                });
                                return LRESULT(1);
                            }
                            PENDING_UP.store(up_code, Ordering::SeqCst);
                            run_action(&b.action);
                            return LRESULT(1);
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// 键盘低级钩子：空格组合 + 识别模式下的 Esc 取消
unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        use windows::Win32::UI::WindowsAndMessaging::{KBDLLHOOKSTRUCT, LLKHF_INJECTED};
        let msg = wp.0 as u32;
        let st = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let vk = st.vkCode;
        // 自己 SendInput 发出去的事件一律放行 —— 否则补发的空格会被自己再吞一次。
        if st.flags.contains(LLKHF_INJECTED) {
            return CallNextHookEx(None, code, wp, lp);
        }
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        // 空格的物理按下 / 抬起：钩子看得见每一条空格事件（包括我们吞掉的），
        // 所以自己记的状态永远和手一致 —— 组合的唯一判据（两个键同时按着）就靠它。
        // 必须排在下面那些 return 之前，`SPACE_MUTED` 吞抬起时才不会漏记。
        if vk == VK_SPACE_CODE {
            SPACE_DOWN.store(is_down, Ordering::SeqCst);
        }

        // 识别信号期间不碰空格：用户可能正在别的窗口打字
        if vk == VK_SPACE_CODE && !capture_active() {
            // 组合已成立 → 这一下空格彻底作废（自动重复、抬起都吞）
            if SPACE_MUTED.load(Ordering::SeqCst) {
                if is_up {
                    SPACE_MUTED.store(false, Ordering::SeqCst);
                }
                return LRESULT(1);
            }
            if is_down {
                // ① 有「空格组合」的鼠标键正按着 → 组合成立（唯一条件：两个键同时按着）→
                //    立刻触发，并把这一下空格吞掉。必须排在"扣空格"之前，
                //    否则空格会先被扣下、永远等不到触发。
                let held = chord_button_held();
                if held != 0 {
                    let btn = match held {
                        1 => "middle",
                        2 => "x1",
                        _ => "x2",
                    };
                    let (c, s, a, _) = mods_down();
                    if let Some(b) = match_chord_binding(btn, c, s, a) {
                        mute_space();
                        run_action(&b.action);
                        return LRESULT(1);
                    }
                }
                // ② 空格先按住：扣下不发给系统，看后面有没有鼠标键来接。
                //    自动重复（用户是按住而不是点一下）→ 立刻补发按下，之后照常放行；
                //    这条同时也兜住"抬起事件丢了"的意外 —— 状态会自己解开。
                if SPACE_ARMED.load(Ordering::SeqCst) {
                    SPACE_ARMED.store(false, Ordering::SeqCst);
                    SPACE_REPLAYED.store(true, Ordering::SeqCst);
                    send_keys(&[(VK_SPACE_CODE, false)]);
                    return LRESULT(1);
                }
                if HAS_CHORD.load(Ordering::SeqCst) && !SPACE_REPLAYED.load(Ordering::SeqCst) {
                    // 先扣下：抬起时若没凑成组合，再原样补发出去。
                    // SPACE_REPLAYED 为真表示已经判定过"这是按住不放"（补发过了），
                    // 后面的自动重复就别再扣一遍了，否则会一直"吞一次、放一次"地抖。
                    SPACE_ARMED.store(true, Ordering::SeqCst);
                    return LRESULT(1);
                }
            } else if is_up {
                // 一直到松手都没等到鼠标键 → 这一下就是普通空格，原样补发出去
                if SPACE_ARMED.swap(false, Ordering::SeqCst) {
                    SPACE_REPLAYED.store(false, Ordering::SeqCst);
                    send_keys(&[(VK_SPACE_CODE, false), (VK_SPACE_CODE, true)]);
                    return LRESULT(1);
                }
                // 配对前面补发的那次按下
                if SPACE_REPLAYED.swap(false, Ordering::SeqCst) {
                    return CallNextHookEx(None, code, wp, lp);
                }
            }
        } else if is_down && SPACE_ARMED.load(Ordering::SeqCst) {
            // 扣着空格的时候按了别的键 → 先把空格补出去，保证按键顺序不乱
            SPACE_ARMED.store(false, Ordering::SeqCst);
            SPACE_REPLAYED.store(true, Ordering::SeqCst);
            send_keys(&[(VK_SPACE_CODE, false)]);
        }

        // 识别模式：键盘这边只认「Esc 取消」，其余键照常放行（鼠标键由 mouse_proc 处理）
        if is_down && capture_active() && vk == 0x1B {
            end_capture();
            if let Some(app) = APP.get() {
                emit_json(app, "mouse-capture-cancel", serde_json::json!(null));
            }
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

/// 启动时挂载钩子：独立线程 + 消息泵（低级钩子要求装钩线程泵消息）。
/// 总开关关闭时钩子一律放行，开销为零；这样「识别信号」在关闭状态下也能用。
pub fn install(app: AppHandle) {
    let _ = APP.set(app);
    std::thread::spawn(|| {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetMessageW, MSG, SetWindowsHookExW, WH_KEYBOARD_LL, WH_MOUSE_LL,
        };
        unsafe {
            let h_mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
            let h_kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), None, 0);
            if h_mouse.is_err() || h_kb.is_err() {
                crate::log_msg("mouse hotkey hooks install failed");
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        }
    });
}
