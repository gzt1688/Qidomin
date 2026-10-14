//! 本地文件名索引 + 快速搜索。
//!
//! 思路：后台线程扫描「常用目录」（桌面 / 文档 / 下载 / 图片 / 视频 / 音乐 / 开始菜单 /
//! 各分区链接目录）或全部固定盘，把 (path, name, ...) 落进 SQLite 的 `files_index` 表；
//! 前端输入即查、毫秒级返回，比依赖 Windows 搜索服务（常关着 / 很慢）快得多。

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use tauri::Manager;

use crate::commands::AppSettings;
use crate::db::Db;

/// 全盘扫描时跳过的系统 / 缓存目录（小写名匹配，避免扫 Windows、node_modules 等）。
const SKIP_DIRS: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "$recycle.bin",
    "system volume information",
    "node_modules",
    ".git",
    "appdata",
];

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub ext: String,
    pub size: i64,
    pub mtime: i64,
}

fn mtime_of(p: &Path) -> i64 {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn size_of(p: &Path) -> i64 {
    p.metadata().map(|m| m.len() as i64).unwrap_or(0)
}

/// 递归扫描一个目录，结果追加到 `out`。
/// `depth` 限制递归深度，避免符号链接 / 极深目录把栈跑爆。
fn scan_dir(dir: &Path, recursive: bool, out: &mut Vec<FileEntry>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // 跳过隐藏项（.git、配置文件等），减少噪声
        if name.starts_with('.') {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_dir() {
            if SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
                continue;
            }
            out.push(FileEntry {
                path: path.to_string_lossy().into_owned(),
                name,
                is_dir: true,
                ext: String::new(),
                size: 0,
                mtime: mtime_of(&path),
            });
            if recursive && depth < 24 {
                scan_dir(&path, true, out, depth + 1);
            }
        } else {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            out.push(FileEntry {
                path: path.to_string_lossy().into_owned(),
                name,
                is_dir: false,
                ext,
                size: size_of(&path),
                mtime: mtime_of(&path),
            });
        }
    }
}

/// 收集需要索引的「常用目录」。
pub(crate) fn common_dirs(db: &Db) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let push = |d: Option<PathBuf>, dirs: &mut Vec<PathBuf>| {
        if let Some(d) = d {
            if d.is_dir() {
                dirs.push(d);
            }
        }
    };
    push(dirs::desktop_dir(), &mut dirs);
    push(dirs::document_dir(), &mut dirs);
    push(dirs::download_dir(), &mut dirs);
    push(dirs::picture_dir(), &mut dirs);
    push(dirs::video_dir(), &mut dirs);
    push(dirs::audio_dir(), &mut dirs);
    // 开始菜单（用户级）：程序快捷方式都在这
    if let Some(data) = dirs::data_dir() {
        let sm = data
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        push(Some(sm), &mut dirs);
    }
    // 开始菜单（公共级）
    let common_sm = PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs");
    push(Some(common_sm), &mut dirs);
    // 各分区的链接目录（用户把文件归到分区里的真实位置）
    if let Ok(links) = db.list_linked_folders() {
        for (_, p) in links {
            let pb = PathBuf::from(&p);
            if pb.is_dir() {
                dirs.push(pb);
            }
        }
    }
    dirs
}

/// 枚举所有「可读盘符」（A-Z）。全盘索引是用户手动开启的重操作，
/// 这里不严格区分盘类型——能读根目录就纳入（U 盘 / 网络盘通常用户不会挂，且可接受）。
fn fixed_drives() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for c in b'A'..=b'Z' {
        let root = format!("{}:\\", c as char);
        if std::fs::read_dir(&root).is_ok() {
            out.push(PathBuf::from(root));
        }
    }
    out
}

fn to_rows(entries: &[FileEntry]) -> Vec<(String, String, i64, String, i64, i64)> {
    entries
        .iter()
        .map(|e| {
            (
                e.path.clone(),
                e.name.clone(),
                if e.is_dir { 1 } else { 0 },
                e.ext.clone(),
                e.size,
                e.mtime,
            )
        })
        .collect()
}

/// 重建索引：扫常用目录（必做），若 `full` 再扫全盘。返回 (条目数, 耗时秒)。
pub fn rebuild_index(db: &Db, full: bool) -> (usize, u64) {
    let start = std::time::Instant::now();
    let mut common = Vec::new();
    for d in common_dirs(db) {
        scan_dir(&d, true, &mut common, 0);
    }
    let common_n = common.len();
    let _ = db.rebuild_file_index("common", &to_rows(&common));

    let mut total = common_n;
    if full {
        let mut full_entries = Vec::new();
        for d in fixed_drives() {
            scan_dir(&d, true, &mut full_entries, 0);
        }
        let _ = db.rebuild_file_index("full", &to_rows(&full_entries));
        total += full_entries.len();
    }
    let secs = start.elapsed().as_secs();
    (total, secs)
}

/// 返回所有「可读盘符」（大写，如 "C"）。
pub fn drive_letters() -> Vec<String> {
    let mut out = Vec::new();
    for c in b'A'..=b'Z' {
        let root = format!("{}:\\", c as char);
        if std::fs::read_dir(&root).is_ok() {
            out.push((c as char).to_string());
        }
    }
    out
}

/// 重建单个盘的索引（scope = 盘符大写）。返回 (条目数, 耗时秒)。
pub fn rebuild_drive(db: &Db, letter: &str) -> (usize, u64) {
    let start = std::time::Instant::now();
    let root = format!("{letter}:\\");
    let mut entries = Vec::new();
    let p = PathBuf::from(&root);
    if p.is_dir() {
        scan_dir(&p, true, &mut entries, 0);
    }
    let n = entries.len();
    let _ = db.rebuild_file_index(letter, &to_rows(&entries));
    (n, start.elapsed().as_secs())
}

/// 从设置读「全盘索引」与「额外盘符」。
pub(crate) fn load_settings(db: &Db) -> (bool, Vec<String>) {
    match db.get_setting("app") {
        Ok(Some(json)) => serde_json::from_str::<AppSettings>(&json)
            .map(|s| (s.full_disk_index, s.index_drives))
            .unwrap_or((false, Vec::new())),
        _ => (false, Vec::new()),
    }
}

/// 按设置重建：常用目录 + 全盘（若开）或勾选的盘符。
pub fn rebuild_selected(db: &Db) -> (usize, u64) {
    let start = std::time::Instant::now();
    let mut common = Vec::new();
    for d in common_dirs(db) {
        scan_dir(&d, true, &mut common, 0);
    }
    let _ = db.rebuild_file_index("common", &to_rows(&common));
    let mut total = common.len();
    let (full, drives) = load_settings(db);
    let list: Vec<String> = if full {
        drive_letters()
    } else {
        drives
    };
    for letter in list {
        let (n, _) = rebuild_drive(db, &letter);
        total += n;
    }
    (total, start.elapsed().as_secs())
}

/// 启动后台索引线程：先建一次索引，之后每 15 分钟增量重建。
/// NTFS 盘由 usn::start_watchers 的 USN Journal 实时维护，周期重建会自动跳过这些盘。
pub fn spawn_indexer(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let db = app.state::<Db>().inner().clone();
        // 先让路：启动瞬间把 CPU 让给分区窗口创建，索引晚一两秒无感
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let (n, secs) = rebuild_selected(&db);
        crate::log_msg(&format!("搜索索引构建完成：{n} 项，{secs}s"));
        loop {
            std::thread::sleep(std::time::Duration::from_secs(900));
            let (n, secs) = rebuild_unwatched(&db);
            if n > 0 {
                crate::log_msg(&format!("搜索索引重建完成：{n} 项，{secs}s"));
            }
        }
    });
}

/// 所有「常用目录」是否都被 USN 实时监听覆盖（目录所在盘都有活的 USN 监听）。
///
/// 覆盖了的话，盘级监听会把常用目录的增删改实时写回 `common` scope，
/// 周期任务就没必要再整目录树重扫 + 整表重写了（那是最重的一步）。
fn common_covered_by_usn(db: &Db) -> bool {
    let dirs = common_dirs(db);
    if dirs.is_empty() {
        return false;
    }
    dirs.iter().all(|d| {
        // 路径形如 "C:\..." 才有盘符可言；UNC（\\server\share）等交给兜底扫描
        match d.to_string_lossy().chars().next() {
            Some(c) if c.is_ascii_alphabetic() => {
                crate::usn::is_usn_watching(&c.to_ascii_uppercase().to_string())
            }
            _ => false,
        }
    })
}

/// 周期重建：常用目录必做（兜底）；已由 USN 实时监听的 NTFS 盘跳过，其余盘照扫。
/// 非 NTFS 盘（FAT / 网络盘）/ USN 不可用（无管理员权限）时，这里就是它们的增量兜底。
///
/// 优化：常用目录若**全部**被 USN 覆盖，直接跳过 —— 以前每 15 分钟都要把
/// 桌面/文档/下载/开始菜单整棵树重扫一遍、再把 `common` scope 整表删了重写，
/// 数据全由 USN 实时维护时这是纯浪费（FTS 索引的白白抖动）。
pub fn rebuild_unwatched(db: &Db) -> (usize, u64) {
    let start = std::time::Instant::now();
    let mut total = 0usize;
    if !common_covered_by_usn(db) {
        let mut common = Vec::new();
        for d in common_dirs(db) {
            scan_dir(&d, true, &mut common, 0);
        }
        let _ = db.rebuild_file_index("common", &to_rows(&common));
        total += common.len();
    }
    let (full, drives) = load_settings(db);
    let list: Vec<String> = if full {
        drive_letters()
    } else {
        drives
    };
    for letter in list {
        if crate::usn::is_usn_watching(&letter) {
            continue;
        }
        let (n, _) = rebuild_drive(db, &letter);
        total += n;
    }
    (total, start.elapsed().as_secs())
}
