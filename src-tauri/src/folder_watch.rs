//! 实时监控「关联文件夹」分区指向的真实目录：里面文件增删/改名时，
//! 立刻通知对应分区面板刷新（前端 listen("box-files-changed") 触发 load()）。
//!
//! 用 `notify` 给每个 link_path 挂一个目录监视器（非递归，只看顶层）。
//! 增删/改名事件经过 250ms 去抖后，按"受影响的 box_id"逐个 emit 刷新事件。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use notify::{
    event::ModifyKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use tauri::{AppHandle, Emitter, Manager};

use crate::boxwin::BoxWindows;
use crate::db::Db;

type PathMap = Arc<Mutex<HashMap<String, i64>>>;

pub struct FolderWatcher {
    watcher: Mutex<Option<RecommendedWatcher>>,
    /// 目录路径（全小写）-> box_id
    map: PathMap,
    /// box_id -> 目录路径（全小写），用于反查删除/改链
    by_id: Mutex<HashMap<i64, String>>,
}

/// 路径归一：转字符串并全小写（Windows 不区分大小写，便于匹配事件路径）
fn norm(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// 通知某个分区面板刷新（复用 notify_box_changed 的逻辑）
fn emit_refresh(app: &AppHandle, id: i64) {
    if let Some(label) = app.state::<BoxWindows>().label_for(id) {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.emit("box-files-changed", id);
        }
    }
}

impl FolderWatcher {
    pub fn new(app: AppHandle) -> Self {
        let map: PathMap = Arc::new(Mutex::new(HashMap::new()));
        let by_id: Mutex<HashMap<i64, String>> = Mutex::new(HashMap::new());

        let (tx, rx) = std::sync::mpsc::channel::<i64>();
        let m = map.clone();

        // notify 事件回调：只关心"文件增删/改名"这几类会改变目录清单的事件，
        // 落盘式修改（Modify(Data)）不会改变顶层清单，跳过以减少无谓刷新。
        let handler = move |res: notify::Result<Event>| {
            let ev = match res {
                Ok(ev) => ev,
                Err(_) => return,
            };
            let refresh = matches!(ev.kind, EventKind::Create(_) | EventKind::Remove(_))
                || matches!(ev.kind, EventKind::Modify(ModifyKind::Name(_)));
            if !refresh {
                return;
            }
            // 锁中毒也要继续：这里 panic 会连带杀死 notify 的监视线程，
            // 结果是所有关联文件夹从此不再刷新，且没有任何提示
            let map = m.lock().unwrap_or_else(|e| e.into_inner());
            let mut ids = HashSet::new();
            for p in &ev.paths {
                let key = norm(p);
                if let Some(id) = map.get(&key) {
                    ids.insert(*id);
                }
                if let Some(parent) = p.parent() {
                    let pkey = norm(parent);
                    if let Some(id) = map.get(&pkey) {
                        ids.insert(*id);
                    }
                }
            }
            drop(map);
            for id in ids {
                let _ = tx.send(id);
            }
        };

        let watcher = notify::recommended_watcher(handler).ok();
        if watcher.is_none() {
            crate::log_msg("folder_watch: 创建 notify watcher 失败");
        }

        // 去抖 worker：把 250ms 内同一分区的多次事件合并成一次刷新
        std::thread::spawn(move || {
            let app = app;
            loop {
                let first = match rx.recv() {
                    Ok(id) => id,
                    Err(_) => break, // 通道关闭（应用退出）
                };
                let mut batch = HashSet::new();
                batch.insert(first);
                // 固定截止时间（不是滑动窗口）：滑动窗口下只要目录持续有事件
                // （下载中、批量拷贝），250ms 会被不断重置，刷新一次都发不出去。
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
                loop {
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match rx.recv_timeout(deadline - now) {
                        Ok(extra) => {
                            batch.insert(extra);
                        }
                        Err(_) => break,
                    }
                }
                for id in batch {
                    emit_refresh(&app, id);
                }
            }
        });

        FolderWatcher {
            watcher: Mutex::new(watcher),
            map,
            by_id,
        }
    }

    /// 给某个 (box_id, link_path) 挂上监视。path 不存在也能调用（先登记，
    /// 等目录出现才会有事件），但 notify 对不存在路径 watch 会失败，记日志即可。
    pub fn add(&self, id: i64, path: &str) {
        let key = path.to_lowercase();
        self.by_id.lock().unwrap_or_else(|e| e.into_inner()).insert(id, key.clone());
        self.map.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), id);
        if let Some(w) = self.watcher.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let p = Path::new(path);
            if let Err(e) = w.watch(p, RecursiveMode::NonRecursive) {
                crate::log_msg(&format!(
                    "folder_watch: 监视 {path} 失败（可能目录不存在）: {e}"
                ));
            }
        }
    }

    /// 取消某个分区的监视（删除分区 / 取消关联时调用）
    pub fn remove(&self, id: i64) {
        let key = self.by_id.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        if let Some(key) = key {
            self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
            if let Some(w) = self.watcher.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let _ = w.unwatch(Path::new(&key));
            }
        }
    }

    /// 改链：先卸旧目录、再挂新目录
    pub fn set_link(&self, id: i64, path: &str) {
        self.remove(id);
        if !path.trim().is_empty() {
            self.add(id, path);
        }
    }

    /// 启动时为**所有分区目录**挂监视。
    ///
    /// - 关联文件夹分区：盯它的 `link_path`；
    /// - 普通分区：盯桌面下那个分区目录（隐藏目录，notify 看得见，与 hidden 属性无关）。
    ///
    /// 以前只盯关联文件夹，普通分区目录没人看 —— 在分区里用右键菜单删掉一个文件，
    /// 后端一个事件都不发，前端只能等下一次交互才刷新（看起来就是"删了还在"）。
    ///
    /// 注意：这里**只在目录已存在时挂**，不会为了监视去建目录 / 改属性 ——
    /// 启动阶段是只读的，不碰用户的磁盘。
    pub fn start_watching_all(&self, db: &Db) {
        // 关联文件夹：用各自的 link_path
        match db.list_linked_folders() {
            Ok(list) => {
                for (id, path) in list {
                    self.add(id, &path);
                }
            }
            Err(e) => crate::log_msg(&format!("folder_watch: 列举关联文件夹失败: {e}")),
        }

        // 普通分区：桌面下同名目录
        let desktop = match crate::engine::desktop_dir() {
            Ok(d) => d,
            Err(e) => {
                crate::log_msg(&format!("folder_watch: 拿不到桌面路径，跳过普通分区: {e}"));
                return;
            }
        };
        match crate::engine::load_boxes_with_id(db) {
            Ok(list) => {
                for (id, name, box_type) in list {
                    if box_type == "folder" {
                        continue; // 关联文件夹上面已经挂过了
                    }
                    let dir = desktop.join(&name);
                    if dir.is_dir() {
                        self.add(id, &dir.to_string_lossy());
                    }
                }
            }
            Err(e) => crate::log_msg(&format!("folder_watch: 列举分区失败: {e}")),
        }
        crate::log_msg(&format!(
            "folder_watch: 共监视 {} 个分区目录",
            self.by_id.lock().unwrap_or_else(|e| e.into_inner()).len()
        ));
    }
}
