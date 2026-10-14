//! 分区「吸附同类文件」的后台部分。
//!
//! 手动吸附（点确定时立刻整理一次）由 `commands::set_absorb_cfg` 完成；
//! 这里只负责**自动吸附**：周期性扫描桌面，把新出现的同类型文件收进对应分区。
//!
//! **没有分区开启自动吸附时线程会挂起**（等 [`wake`] 唤醒，或最多 5 秒补查一次），
//! 不再每 2 秒空转查库 —— 绝大多数用户根本没开这个功能，不该为它付出常驻开销。

use crate::boxwin::BoxWindows;
use crate::db::Db;
use crate::engine;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const POLL_INTERVAL: Duration = Duration::from_millis(2000);
/// 空闲时的最长挂起时间：即使唤醒信号丢失，也最多 5 秒补查一次
const IDLE_WAIT: Duration = Duration::from_secs(5);

static WAKE: Mutex<bool> = Mutex::new(false);
static WAKE_CV: Condvar = Condvar::new();

/// 自动吸附配置发生变化（开关、增删分区、导入布局）时调用：唤醒后台线程重新判断。
pub fn wake() {
    if let Ok(mut f) = WAKE.lock() {
        *f = true;
    }
    WAKE_CV.notify_all();
}

pub fn spawn_loop(app: AppHandle) {
    std::thread::spawn(move || loop {
        // 应用退出过程中 state 可能已销毁：用 try_state 而不是 state，否则会 panic
        let Some(db) = app.try_state::<Db>() else {
            break;
        };
        let ids = db.auto_absorb_boxes().unwrap_or_default();
        if ids.is_empty() {
            // 没有任何分区开启自动吸附：挂起线程，别再空转
            let mut flag = WAKE.lock().unwrap_or_else(|e| e.into_inner());
            if *flag {
                *flag = false;
                drop(flag);
                continue;
            }
            let (g, _) = WAKE_CV
                .wait_timeout(flag, IDLE_WAIT)
                .unwrap_or_else(|e| e.into_inner());
            drop(g);
            continue;
        }

        let mut touched: Vec<i64> = Vec::new();
        for id in ids {
            let moved = engine::absorb_into_box(db.inner(), id)
                .map(|r| r.moved)
                .unwrap_or(0);
            if moved > 0 {
                touched.push(id);
            }
        }

        // 收进来之后让对应的分区面板刷新
        if !touched.is_empty() {
            if let Some(bw) = app.try_state::<BoxWindows>() {
                for id in touched {
                    let Some(label) = bw.label_for(id) else { continue };
                    if let Some(win) = app.get_webview_window(&label) {
                        let _ = win.emit("box-files-changed", id);
                    }
                }
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    });
}
