//! 桌面「伤寒论」组件：内置宋本全本（398 条，太阳/阳明/少阳/太阴/少阴/厥阴/
//! 霍乱/阴阳易差后劳复），随机展示条文 + 按方证/内容搜索，独立透明窗口。
//!
//! 内容与搜索全部在前端本地完成（内置 JSON 数据），后端只负责：开关状态读写、
//! 独立窗口的创建/隐藏/位置记忆，以及拖动松手后的吸附对齐。

use crate::boxwin;
use crate::db::{Db, ShanghanWindowCfg};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

pub const SHANGHAN_LABEL: &str = "shanghan";
/// 组件开关：设置页“组件 → 伤寒论”的勾选状态
const ENABLED_KEY: &str = "shanghanEnabled";
/// 开关变化事件名（发给设置窗口，用于同步勾选框）
const ENABLED_EVENT: &str = "shanghan-enabled-changed";

static LAST_SAVE: Mutex<Option<Instant>> = Mutex::new(None);
static DRAGGING: AtomicBool = AtomicBool::new(false);

// ---------- 开关 ----------

pub fn is_enabled(db: &Db) -> bool {
    db.get_setting(ENABLED_KEY)
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// 显示 / 隐藏伤寒论面板
pub fn set_enabled(app: &AppHandle, db: &Db, enabled: bool) -> Result<(), String> {
    db.set_setting(ENABLED_KEY, if enabled { "true" } else { "false" })?;
    if enabled {
        let win = open_shanghan_window(app, db)?;
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        persist_cfg(&win, db);
    } else if let Some(win) = app.get_webview_window(SHANGHAN_LABEL) {
        crate::boxwin::hide_win(&win);
        persist_cfg(&win, db);
    }
    let _ = app.emit(ENABLED_EVENT, enabled);
    Ok(())
}

// ---------- 窗口 ----------

/// 位置/大小节流持久化（拖动/缩放时高频触发）
fn persist_cfg(win: &WebviewWindow, db: &Db) {
    if let Ok(mut last) = LAST_SAVE.lock() {
        let now = Instant::now();
        if let Some(t) = *last {
            if now.duration_since(t) < Duration::from_millis(400) {
                return;
            }
        }
        *last = Some(now);
    }
    force_persist_cfg(win, db);
}

fn force_persist_cfg(win: &WebviewWindow, db: &Db) {
    let Ok(pos) = win.outer_position() else { return };
    let Ok(size) = win.outer_size() else { return };
    let mut cfg = db
        .get_shanghan_window_cfg()
        .ok()
        .flatten()
        .unwrap_or_else(ShanghanWindowCfg::new);
    cfg.x = pos.x;
    cfg.y = pos.y;
    cfg.w = size.width as i32;
    cfg.h = size.height as i32;
    cfg.visible = win.is_visible().unwrap_or(true);
    let _ = db.set_shanghan_window_cfg(&cfg);
}

/// 创建（或复用）伤寒论面板窗口
pub fn open_shanghan_window(app: &AppHandle, db: &Db) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(SHANGHAN_LABEL) {
        return Ok(w);
    }
    let cfg = db
        .get_shanghan_window_cfg()
        .ok()
        .flatten()
        .unwrap_or_else(ShanghanWindowCfg::new);
    let url = WebviewUrl::App("index.html?shanghan=1".into());
    let win = WebviewWindowBuilder::new(app, SHANGHAN_LABEL, url)
        .title("伤寒论")
        .inner_size(cfg.w.max(260) as f64, cfg.h.max(220) as f64)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        // 先隐藏建窗，位置校正到物理像素后再显示，避免在高 DPI 上闪一下
        .visible(false)
        .build()
        .map_err(|e| format!("创建伤寒论窗口失败: {e}"))?;

    // 配置里存的是 outer_position / outer_size 的返回值（物理像素），
    // 而 builder 的 position / inner_size 用的是逻辑像素，高 DPI 下两者不等，这里校正一次。
    let _ = win.set_position(PhysicalPosition::new(cfg.x, cfg.y));
    let _ = win.set_size(PhysicalSize::new(
        cfg.w.max(260) as u32,
        cfg.h.max(220) as u32,
    ));
    if cfg.visible {
        let _ = win.show();
    }

    boxwin::force_no_taskbar(&win);
    crate::apply_ui_zoom(&win);
    // 原生边缘拉伸的吸附
    crate::resize_snap::install(&win, SHANGHAN_LABEL);

    let app2 = app.clone();
    let win2 = win.clone();
    let db2 = db.clone();
    win.on_window_event(move |event| match event {
        WindowEvent::Moved(_) => persist_cfg(&win2, &db2),
        WindowEvent::Resized(_) => persist_cfg(&win2, &db2),
        WindowEvent::CloseRequested { api, .. } => {
            // 关闭 = 隐藏并取消组件勾选（与设置页状态保持一致）
            api.prevent_close();
            crate::boxwin::hide_win(&win2);
            force_persist_cfg(&win2, &db2);
            let _ = db2.set_setting(ENABLED_KEY, "false");
            let _ = app2.emit(ENABLED_EVENT, false);
        }
        _ => {}
    });

    Ok(win)
}

/// 启动时按开关恢复伤寒论面板
pub fn restore_on_startup(app: &AppHandle, db: &Db) {
    if is_enabled(db) {
        if let Err(e) = open_shanghan_window(app, db) {
            crate::log_msg(&format!("open_shanghan_window failed: {e}"));
        }
    }
}

// ---------- 拖动吸附 ----------

/// 面板拖动后的松手吸附（与分区/便签窗口同一套 find_snap 逻辑）
pub fn start_shanghan_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    let win = app
        .get_webview_window(SHANGHAN_LABEL)
        .ok_or_else(|| "伤寒论面板不存在".to_string())?;
    if DRAGGING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    std::thread::spawn(move || {
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                DRAGGING.store(false, Ordering::SeqCst);
            }
        }
        let _g = Guard;

        let mut last: Option<(i32, i32)> = None;
        let mut stable = 0u32;
        let mut ever_moved = force_moved;
        let begin = Instant::now();
        let mut ticks = 0u32;
        loop {
            ticks += 1;
            if ticks > 240 {
                break;
            }
            let Ok(pos) = win.outer_position() else { break };
            let Ok(size) = win.outer_size() else { break };
            if let Some(lp) = last {
                if lp == (pos.x, pos.y) {
                    stable += 1;
                } else {
                    ever_moved = true;
                    stable = 0;
                }
            }
            last = Some((pos.x, pos.y));
            if !ever_moved && begin.elapsed() > Duration::from_millis(1500) {
                break;
            }
            if stable >= 10 {
                if !ever_moved {
                    break;
                }
                // 设置里关了吸附 → 不做松手对齐，直接结束
                if !boxwin::snap_enabled(&app) {
                    break;
                }
                let mut others: Vec<(i64, i32, i32, i32, i32)> = Vec::new();
                if let Some(bw) = app.try_state::<boxwin::BoxWindows>() {
                    for id in bw.ids() {
                        if let Some(l) = bw.label_for(id) {
                            if let Some(w) = app.get_webview_window(&l) {
                                if let (Ok(op), Ok(os)) = (w.outer_position(), w.outer_size()) {
                                    others.push((id, op.x, op.y, os.width as i32, os.height as i32));
                                }
                            }
                        }
                    }
                }
                let threshold = app
                    .state::<Db>()
                    .get_setting("snapThreshold")
                    .ok()
                    .flatten()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(20);
                if let Some((_, nx, ny)) = boxwin::find_snap(
                    pos.x,
                    pos.y,
                    size.width as i32,
                    size.height as i32,
                    &others,
                    threshold,
                ) {
                    let _ = win.set_position(PhysicalPosition::new(nx, ny));
                    // 吸附后的最终位置要立刻落盘：Moved 事件有 400ms 节流，
                    // 而吸附正好发生在刚拖完的高发窗口里，不补这一下重启后位置会回退。
                    if let Some(db) = app.try_state::<crate::db::Db>() {
                        force_persist_cfg(&win, &db);
                    }
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
    Ok(())
}
