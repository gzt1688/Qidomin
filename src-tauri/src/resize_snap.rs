//! 原生窗口边缘拉伸的磁性吸附。
//!
//! 这些窗口都是「无边框 + resizable」：窗口四周有一圈系统原生拉伸边框，
//! 用户从边缘拉宽/拉高时走的是系统模态 resize 循环，前端 JS 的吸附
//! （右下角手柄路径）根本不会执行 —— 这就是"拖动有吸附、拉伸没感觉"的根因。
//!
//! 原生拉伸期间系统每帧都发 WM_SIZING，且允许消息处理者修改 proposed 矩形：
//! 在这里按移动中的边缘做磁性吸附。右下角手柄路径的 JS 吸附保持不变，
//! 两条路都有吸附。

#[cfg(target_os = "windows")]
use tauri::{AppHandle, Manager};

/// 子类 ID（与 boxwin 的防最小化子类错开）
#[cfg(target_os = "windows")]
const SUBCLASS_ID: usize = 0x51D0_0002;

#[cfg(target_os = "windows")]
struct RefData {
    app: AppHandle,
    owner: String,
}

/// 给窗口挂 WM_SIZING 吸附。owner = 窗口 label（"box-N" / "notes" / ...），
/// 用于在候选里排除自己。
pub fn install(win: &tauri::WebviewWindow, owner: &str) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::Shell::SetWindowSubclass;
        if let Ok(hwnd) = win.hwnd() {
            let data = Box::new(RefData {
                app: win.app_handle().clone(),
                owner: owner.to_string(),
            });
            unsafe {
                let _ =
                    SetWindowSubclass(hwnd, Some(sizing_proc), SUBCLASS_ID, Box::into_raw(data) as usize);
            }
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn sizing_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
    _id: usize,
    data: usize,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Shell::DefSubclassProc;

    const WM_SIZING: u32 = 0x0214;
    if msg == WM_SIZING && data != 0 {
        let r = &mut *(lparam.0 as *mut windows::Win32::Foundation::RECT);
        let rd = &*(data as *const RefData);
        if apply_snap(rd, wparam.0, r) {
            return windows::Win32::Foundation::LRESULT(1);
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// 吸附阈值：与前端 resizeSnap 同一套 —— 拉伸只有一条边在动，感知比拖动弱，
/// 阈值放大一倍、下限 32 物理像素。总开关关闭或阈值为 0 → None（不吸附）。
#[cfg(target_os = "windows")]
fn snap_threshold(app: &AppHandle) -> Option<i32> {
    if !crate::boxwin::snap_enabled(app) {
        return None;
    }
    let base = app
        .state::<crate::db::Db>()
        .get_setting("snapThreshold")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(20);
    if base <= 0 {
        return None;
    }
    Some((base * 2).max(32))
}

/// 候选矩形 = 其它可见分区 + 其它可见组件 + 自己所在屏幕的工作区（物理像素）。
/// 与 boxwin::snap_targets 同源，排除自己。
#[cfg(target_os = "windows")]
fn collect_candidates(app: &AppHandle, owner: &str) -> Vec<[i32; 4]> {
    let mut rects: Vec<[i32; 4]> = Vec::new();
    let push = |label: &str, rects: &mut Vec<[i32; 4]>| {
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
    if let Some(bw) = app.try_state::<crate::boxwin::BoxWindows>() {
        for id in bw.ids() {
            if let Some(l) = bw.label_for(id) {
                push(&l, &mut rects);
            }
        }
    }
    for l in [
        crate::notes::NOTE_LABEL,
        crate::stock::STOCK_LABEL,
        crate::huangli::HUANGLI_LABEL,
        crate::poetry::POETRY_LABEL,
    ] {
        push(l, &mut rects);
    }
    if let Some(win) = app.get_webview_window(owner) {
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
    rects
}

/// val 距离最近的候选边（阈值内）
#[cfg(target_os = "windows")]
fn nearest(val: i32, edges: &[i32], th: i32) -> Option<i32> {
    let mut best = None;
    let mut bd = th;
    for &e in edges {
        let d = (val - e).abs();
        if d < bd {
            bd = d;
            best = Some(e);
        }
    }
    best
}

/// WM_SIZING：按移动中的边缘吸附 proposed 矩形。有修改返回 true。
#[cfg(target_os = "windows")]
fn apply_snap(
    rd: &RefData,
    wmsz: usize,
    r: &mut windows::Win32::Foundation::RECT,
) -> bool {
    // WMSZ_LEFT=1 RIGHT=2 TOP=3 TOPLEFT=4 TOPRIGHT=5 BOTTOM=6 BOTTOMLEFT=7 BOTTOMRIGHT=8
    let right_moving = matches!(wmsz, 2 | 5 | 8);
    let left_moving = matches!(wmsz, 1 | 4 | 7);
    let bottom_moving = matches!(wmsz, 6 | 7 | 8);
    let top_moving = matches!(wmsz, 3 | 4 | 5);
    if !(right_moving || left_moving || bottom_moving || top_moving) {
        return false;
    }
    let Some(th) = snap_threshold(&rd.app) else {
        return false;
    };
    let targets = collect_candidates(&rd.app, &rd.owner);
    if targets.is_empty() {
        return false;
    }
    let xs: Vec<i32> = targets.iter().flat_map(|t| [t[0], t[0] + t[2]]).collect();
    let ys: Vec<i32> = targets.iter().flat_map(|t| [t[1], t[1] + t[3]]).collect();

    let (ol, ot, orr, ob) = (r.left, r.top, r.right, r.bottom);
    let (mut nl, mut nt, mut nr, mut nb) = (ol, ot, orr, ob);
    // 防止吸附把矩形挤翻：贴齐后至少保留 80×60 的面积
    if right_moving {
        if let Some(e) = nearest(orr, &xs, th) {
            if e - ol >= 80 {
                nr = e;
            }
        }
    }
    if left_moving {
        if let Some(e) = nearest(ol, &xs, th) {
            if orr - e >= 80 {
                nl = e;
            }
        }
    }
    if bottom_moving {
        if let Some(e) = nearest(ob, &ys, th) {
            if e - ot >= 60 {
                nb = e;
            }
        }
    }
    if top_moving {
        if let Some(e) = nearest(ot, &ys, th) {
            if ob - e >= 60 {
                nt = e;
            }
        }
    }
    let changed = nl != ol || nt != ot || nr != orr || nb != ob;
    if changed {
        r.left = nl;
        r.top = nt;
        r.right = nr;
        r.bottom = nb;
    }
    changed
}

#[cfg(not(target_os = "windows"))]
pub fn install(_win: &tauri::WebviewWindow, _owner: &str) {}
