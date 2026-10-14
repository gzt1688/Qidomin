//! 桌面便签组件：透明便签窗口（可拖动 / 缩放 / 与分区吸附 / 换背景）+ 到点提醒。
//!
//! 提醒语义：
//!   - 有闹钟（remind_alarm = true）：到点播放系统提示音，并让便签窗口显示出来；
//!   - 无闹钟（remind_alarm = false）：到点只做静默提示（便签窗口内高亮 + 顶部横幅），不发声。
//! 提醒由后台线程每 15 秒轮询数据库触发，触发后写入 fired 标记，避免重复提醒；
//! 即使窗口当时不在，重新打开也能看到“已提醒”状态。

use crate::boxwin;
use crate::db::{Db, NoteWindowCfg};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

pub const NOTE_LABEL: &str = "notes";
/// 组件开关：设置页“组件 → 便签”的勾选状态
const ENABLED_KEY: &str = "notesEnabled";
/// 提醒事件名（发给便签窗口，用于弹出横幅）
const REMIND_EVENT: &str = "note-remind";
/// 开关变化事件名（发给设置窗口，用于同步勾选框）
const ENABLED_EVENT: &str = "notes-enabled-changed";

static LAST_SAVE: Mutex<Option<Instant>> = Mutex::new(None);
static DRAGGING: AtomicBool = AtomicBool::new(false);

fn system_root() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())
}

// ---------- 开关 ----------

pub fn is_enabled(db: &Db) -> bool {
    db.get_setting(ENABLED_KEY)
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// 显示 / 隐藏便签组件
pub fn set_enabled(app: &AppHandle, db: &Db, enabled: bool) -> Result<(), String> {
    db.set_setting(ENABLED_KEY, if enabled { "true" } else { "false" })?;
    if enabled {
        let win = open_note_window(app, db)?;
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        persist_cfg(&win, db);
    } else if let Some(win) = app.get_webview_window(NOTE_LABEL) {
        crate::boxwin::hide_win(&win);
        persist_cfg(&win, db);
    }
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
        .get_note_window_cfg()
        .ok()
        .flatten()
        .unwrap_or_else(NoteWindowCfg::new);
    cfg.x = pos.x;
    cfg.y = pos.y;
    cfg.w = size.width as i32;
    cfg.h = size.height as i32;
    cfg.visible = win.is_visible().unwrap_or(true);
    let _ = db.set_note_window_cfg(&cfg);
}

/// 创建（或复用）便签窗口
pub fn open_note_window(app: &AppHandle, db: &Db) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(NOTE_LABEL) {
        return Ok(w);
    }
    let cfg = db
        .get_note_window_cfg()
        .ok()
        .flatten()
        .unwrap_or_else(NoteWindowCfg::new);
    let url = WebviewUrl::App("index.html?notes=1".into());
    // 先隐藏建窗：配置里存的是物理像素，builder 的 position / inner_size 用逻辑像素，
    // 高 DPI 屏上两者不等，直接塞进去重启后位置会漂移。建好后用物理像素校正。
    let win = WebviewWindowBuilder::new(app, NOTE_LABEL, url)
        .title("便签")
        .inner_size(cfg.w.max(220) as f64, cfg.h.max(160) as f64)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        .visible(false)
        .build()
        .map_err(|e| format!("创建便签窗口失败: {e}"))?;
    let _ = win.set_position(PhysicalPosition::new(cfg.x, cfg.y));
    let _ = win.set_size(PhysicalSize::new(
        cfg.w.max(220) as u32,
        cfg.h.max(160) as u32,
    ));
    if cfg.visible {
        let _ = win.show();
    }

    boxwin::force_no_taskbar(&win);
    crate::apply_ui_zoom(&win);
    // 原生边缘拉伸的吸附
    crate::resize_snap::install(&win, NOTE_LABEL);

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

/// 启动时按开关恢复便签窗口
pub fn restore_on_startup(app: &AppHandle, db: &Db) {
    if is_enabled(db) {
        if let Err(e) = open_note_window(app, db) {
            crate::log_msg(&format!("open_note_window failed: {e}"));
        }
    }
}

// ---------- 拖动吸附 ----------

/// 便签窗口拖动后的松手吸附（与分区窗口同一套 find_snap 逻辑）
pub fn start_note_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    let win = app
        .get_webview_window(NOTE_LABEL)
        .ok_or_else(|| "便签窗口不存在".to_string())?;
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

// ---------- 提醒 ----------

/// 播放提醒音（有闹钟的便签）
fn play_alarm() {
    std::thread::spawn(|| {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_FILENAME, SND_FLAGS};

        let media = format!(r"{}\Media", system_root());
        let file = [
            "Alarm01.wav",
            "Alarm03.wav",
            "Windows Notify System Generic.wav",
            "notify.wav",
            "chimes.wav",
        ]
        .iter()
        .map(|n| format!("{media}\\{n}"))
        .find(|p| Path::new(p).exists());

        let (target, flags) = match file {
            Some(p) => (
                HSTRING::from(p.as_str()),
                SND_FLAGS(SND_ASYNC.0 | SND_FILENAME.0),
            ),
            // 没找到系统音频文件时退回系统提示音别名
            None => (
                HSTRING::from("SystemExclamation"),
                SND_FLAGS(SND_ASYNC.0 | SND_ALIAS.0),
            ),
        };
        // 响 4 次，约 5 秒，够醒目又不会一直吵
        for _ in 0..4 {
            unsafe {
                let _ = PlaySoundW(PCWSTR(target.as_ptr()), None, flags);
            }
            std::thread::sleep(Duration::from_millis(1400));
        }
    });
}

/// 后台提醒线程：每 15 秒检查一次到点便签
pub fn spawn_reminder_loop(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(15));
        let Some(db) = app.try_state::<Db>().map(|s| s.inner().clone()) else {
            continue;
        };
        // 组件关闭时不提醒（数据仍保留，重新打开后到点会补提醒）
        if !is_enabled(&db) {
            continue;
        }
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let Ok(due) = db.due_notes(&now) else { continue };
        for note in due {
            let _ = db.mark_note_fired(note.id);
            if note.remind_alarm {
                play_alarm();
            }
            // 便签窗口可能被隐藏，先确保它显示出来（窗口创建/显示统一投递到主线程）
            let h = app.clone();
            let db2 = db.clone();
            let _ = app.run_on_main_thread(move || {
                if let Ok(win) = open_note_window(&h, &db2) {
                    let _ = win.show();
                }
            });
            let _ = app.emit_to(NOTE_LABEL, REMIND_EVENT, &note);
        }
    });
}
