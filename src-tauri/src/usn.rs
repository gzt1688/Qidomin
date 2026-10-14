//! 实时文件监听：NTFS 走 USN Journal（递归实时），其余回退浅层变更通知。
//!
//! - **USN Journal（NTFS）**：先 `FSCTL_QUERY_USN_JOURNAL` 记下 NextUsn，再
//!   `FSCTL_ENUM_USN_DATA` 枚举整卷 MFT 建出「文件引用号 → (父引用号, 名字, 是否目录)」
//!   映射（枚举期间的变更由从 NextUsn 起的读取补上，无空窗），然后
//!   `FSCTL_READ_USN_JOURNAL` 阻塞式读记录：建/删/改名即时写回 FTS5 索引，
//!   内容写完（CLOSE + 数据变化）时刷新 size/mtime。路径由映射逐级上溯拼出。
//!   打开卷句柄（`\\.\C:`）通常需要管理员权限，失败时自动回退（见下）。
//! - **回退（非 NTFS / 无权限 / 枚举失败）**：用 ReadDirectoryChangesW 只监听各根目录的
//!   **直接子项**（bWatchSubtree = FALSE，保证路径精确还原、不会污染索引），深层嵌套的
//!   变更由 search::spawn_indexer 的周期重建兜底（rebuild_unwatched 会跳过已 USN 监听的盘）。

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, UNIX_EPOCH};

use tauri::Manager;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetVolumeInformationW, ReadDirectoryChangesW, FILE_LIST_DIRECTORY,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    CREATE_USN_JOURNAL_DATA, FSCTL_CREATE_USN_JOURNAL, FSCTL_ENUM_USN_DATA,
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, MFT_ENUM_DATA_V0,
    READ_USN_JOURNAL_DATA_V1, USN_JOURNAL_DATA_V0,
};

use crate::db::Db;
use crate::search::{common_dirs, load_settings};

// ---------- USN 记录原因位（winnt.h） ----------
const USN_DATA_OVERWRITE: u32 = 0x0000_0001;
const USN_DATA_EXTEND: u32 = 0x0000_0002;
const USN_FILE_CREATE: u32 = 0x0000_0100;
const USN_FILE_DELETE: u32 = 0x0000_0200;
const USN_RENAME_OLD: u32 = 0x0000_1000;
const USN_RENAME_NEW: u32 = 0x0000_2000;
const USN_BASIC_INFO: u32 = 0x0000_8000;
const USN_CLOSE: u32 = 0x8000_0000;
const FILE_ATTR_DIR: u32 = 0x10;

// ---------- 浅层回退（ReadDirectoryChangesW）用的常量 ----------
const BUF_LEN: usize = 256 * 1024;
const ACTION_ADDED: u32 = 1;
const ACTION_REMOVED: u32 = 2;
const ACTION_MODIFIED: u32 = 3;
const ACTION_RENAMED_OLD: u32 = 4;
const ACTION_RENAMED_NEW: u32 = 5;

/// 浅层回退监听注册表（根路径 → 是否已挂监听）
static WATCHED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
fn watched() -> &'static Mutex<std::collections::HashSet<String>> {
    WATCHED.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// USN 监听注册表（盘符 → 是否已挂 USN 监听；启动失败会撤销）
static USN_WATCHED: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
fn usn_watched() -> &'static Mutex<std::collections::HashSet<String>> {
    USN_WATCHED.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// 某盘是否已由 USN Journal 实时维护（周期重建据此跳过该盘）。
pub fn is_usn_watching(letter: &str) -> bool {
    usn_watched()
        .lock()
        .map(|w| w.contains(letter))
        .unwrap_or(false)
}

/// 启动所有实时监听（在 search::spawn_indexer 之后调用一次）。
pub fn start_watchers(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // MFT 枚举较吃 CPU，避开启动瞬间的窗口创建，晚几秒开始
        std::thread::sleep(Duration::from_millis(2500));
        let db = app.state::<Db>().inner().clone();
        // 1) 盘符：NTFS → USN Journal 递归实时；否则 → 浅层回退
        let (full, drives) = load_settings(&db);
        let letters: Vec<String> = if full {
            crate::search::drive_letters()
        } else {
            drives
        };
        for letter in letters {
            start_drive(&db, &letter);
        }
        // 2) 常用目录：若所在盘已被 USN 监听则跳过（数据会由盘级监听写回 common scope），
        //    否则挂浅层监听兜底
        for d in common_dirs(&db) {
            if let Some(l) = drive_letter_of(&d) {
                if is_usn_watching(&l) {
                    continue;
                }
            }
            let key = d.to_string_lossy().to_string();
            {
                let mut w = watched().lock().unwrap();
                if w.contains(&key) {
                    continue;
                }
                w.insert(key);
            }
            let db2 = db.clone();
            std::thread::spawn(move || watch_root(db2, d, "common".to_string()));
        }
    });
}

fn drive_letter_of(p: &Path) -> Option<String> {
    let s = p.to_string_lossy();
    let mut it = s.chars();
    let c = it.next()?;
    if it.next() == Some(':') && c.is_ascii_alphabetic() {
        Some(c.to_ascii_uppercase().to_string())
    } else {
        None
    }
}

/// 为一个盘符启动监听：NTFS 尝试 USN，失败/非 NTFS 回退浅层。
fn start_drive(db: &Db, letter: &str) {
    {
        let w = usn_watched().lock().unwrap();
        if w.contains(letter) {
            return;
        }
    }
    let db2 = db.clone();
    let l = letter.to_string();
    if is_ntfs(letter) {
        // 先占位防重复，线程启动失败时撤销
        usn_watched()
            .lock()
            .unwrap()
            .insert(letter.to_string());
        let (full, drives) = load_settings(db);
        let scoped = full || drives.iter().any(|d| d.eq_ignore_ascii_case(letter));
        let commons = common_prefixes_on(db, letter);
        std::thread::spawn(move || {
            if !watch_drive_usn(&db2, &l, scoped, commons) {
                // USN 启动失败（多为无管理员权限）→ 撤销占位，交浅层回退
                usn_watched().lock().unwrap().remove(&l);
                let db3 = db2.clone();
                std::thread::spawn(move || rdc_watch_letter(db3, l));
            }
        });
    } else {
        // 非 NTFS（FAT / exFAT / 网络盘）：浅层回退，深层由周期重建兜底
        std::thread::spawn(move || rdc_watch_letter(db2, l));
    }
}

fn rdc_watch_letter(db: Db, letter: String) {
    watch_root(db, PathBuf::from(format!("{letter}:\\")), letter);
}

/// 常用目录里位于该盘的路径前缀（小写 + 尾部反斜杠），供 common scope 判定。
fn common_prefixes_on(db: &Db, letter: &str) -> Vec<String> {
    let pat = format!("{}:", letter.to_lowercase());
    let mut out = Vec::new();
    for d in common_dirs(db) {
        let s = d.to_string_lossy().to_lowercase();
        if s.starts_with(&pat) {
            out.push(format!("{}\\", s.trim_end_matches('\\')));
        }
    }
    out
}

// ==================== USN Journal（NTFS 递归实时） ====================

pub fn is_ntfs(letter: &str) -> bool {
    let root: Vec<u16> = format!("{letter}:\\")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut fs = [0u16; 32];
    let ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut fs),
        )
    };
    ok.map(|_| {
        let name = String::from_utf16_lossy(&fs);
        name.trim_end_matches('\0').eq_ignore_ascii_case("NTFS")
    })
    .unwrap_or(false)
}

/// 打开卷设备 `\\.\C:`（GENERIC_READ，通常需要管理员权限）。
fn open_volume(letter: &str) -> Result<HANDLE, ()> {
    let wide: Vec<u16> = format!(r"\\.\{letter}:")
        .encode_utf16()
        .chain([0])
        .collect();
    match unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0x8000_0000, // GENERIC_READ
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    } {
        Ok(h) if h != INVALID_HANDLE_VALUE => Ok(h),
        _ => Err(()),
    }
}

/// 查询 USN Journal → (UsnJournalID, NextUsn)。
fn query_journal(h: HANDLE) -> Option<(u64, i64)> {
    let mut out: USN_JOURNAL_DATA_V0 = unsafe { std::mem::zeroed() };
    let mut ret = 0u32;
    let r = unsafe {
        DeviceIoControl(
            h,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(&mut out as *mut _ as *mut c_void),
            std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
            Some(&mut ret),
            None,
        )
    };
    if r.is_ok() {
        Some((out.UsnJournalID, out.NextUsn))
    } else {
        None
    }
}

fn create_journal(h: HANDLE) {
    let cj = CREATE_USN_JOURNAL_DATA {
        MaximumSize: 0x0200_0000,
        AllocationDelta: 0x0080_0000,
    };
    unsafe {
        let _ = DeviceIoControl(
            h,
            FSCTL_CREATE_USN_JOURNAL,
            Some(&cj as *const _ as *const c_void),
            std::mem::size_of::<CREATE_USN_JOURNAL_DATA>() as u32,
            None,
            0,
            None,
            None,
        );
    }
}

/// FSCTL_ENUM_USN_DATA 枚举整卷 MFT → FRS → (父 FRS, 名字, 是否目录)。
/// 只用来还原 USN 记录里的路径（包括被扫描跳过的系统目录，保证上溯链完整）。
fn enum_mft(h: HANDLE) -> Option<HashMap<u64, (u64, String, bool)>> {
    let mut out: HashMap<u64, (u64, String, bool)> = HashMap::new();
    let mut start: u64 = 0;
    let mut buf = vec![0u8; 16 * 1024 * 1024];
    loop {
        let med = MFT_ENUM_DATA_V0 {
            StartFileReferenceNumber: start,
            LowUsn: 0,
            HighUsn: -1i64,
        };
        let mut ret = 0u32;
        let r = unsafe {
            DeviceIoControl(
                h,
                FSCTL_ENUM_USN_DATA,
                Some(&med as *const _ as *const c_void),
                std::mem::size_of::<MFT_ENUM_DATA_V0>() as u32,
                Some(buf.as_mut_ptr() as *mut c_void),
                buf.len() as u32,
                Some(&mut ret),
                None,
            )
        };
        if r.is_err() {
            break;
        }
        let n = ret as usize;
        if n < 8 {
            break;
        }
        // 输出前 8 字节 = 下一次枚举的 StartFileReferenceNumber
        start = u64::from_le_bytes(buf[0..8].try_into().ok()?);
        let mut off = 8usize;
        while off + 4 <= n {
            let reclen = u32::from_le_bytes(buf[off..off + 4].try_into().ok()?) as usize;
            if reclen == 0 || off + reclen > n {
                break;
            }
            let rec = &buf[off..off + reclen];
            let major = u16::from_le_bytes(rec[4..6].try_into().ok()?);
            if major == 2 {
                let frs = u64::from_le_bytes(rec[8..16].try_into().ok()?);
                let parent = u64::from_le_bytes(rec[16..24].try_into().ok()?);
                let attrs = u32::from_le_bytes(rec[52..56].try_into().ok()?);
                let fnlen = u16::from_le_bytes(rec[56..58].try_into().ok()?) as usize;
                let fnoff = u16::from_le_bytes(rec[58..60].try_into().ok()?) as usize;
                if fnoff + fnlen <= reclen {
                    let raw = &rec[fnoff..fnoff + fnlen];
                    let name_w: Vec<u16> = raw
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let name = String::from_utf16_lossy(&name_w);
                    let is_dir = attrs & FILE_ATTR_DIR != 0;
                    out.insert(frs, (parent, name, is_dir));
                }
            }
            off += reclen;
        }
        if n < buf.len() {
            break; // 缓冲未满 → 枚举完成
        }
    }
    Some(out)
}

struct UsnDrive {
    letter: String,
    root: String, // "C:\\"
    entries: HashMap<u64, (u64, String, bool)>,
    drive_scoped: bool,
    common_prefixes: Vec<String>,
}

impl UsnDrive {
    /// 从 (父 FRS, 名字) 上溯到根拼出完整路径。
    fn resolve_from(&self, parent: u64, name: &str) -> Option<String> {
        let mut parts = vec![name.to_string()];
        let mut cur = parent;
        let mut guard = 0;
        loop {
            if cur == 5 || cur == 0 {
                break; // 5 = 卷根目录
            }
            let e = self.entries.get(&cur)?;
            parts.push(e.1.clone());
            cur = e.0;
            guard += 1;
            if guard > 512 {
                return None;
            }
        }
        parts.reverse();
        Some(format!("{}{}", self.root, parts.join("\\")))
    }

    /// 该变更路径属于哪些 scope（盘符 scope + common scope）。
    fn scopes_for(&self, path: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.drive_scoped {
            out.push(self.letter.clone());
        }
        let lower = path.to_lowercase();
        for p in &self.common_prefixes {
            if lower.starts_with(p) {
                out.push("common".to_string());
                break;
            }
        }
        out
    }

    /// 带元数据写回索引（目录无 ext/size）。
    fn upsert(&self, db: &Db, path: &str, name: &str, is_dir: bool, scopes: &[String]) {
        let ext = if is_dir {
            String::new()
        } else {
            match name.rfind('.') {
                Some(i) if i + 1 < name.len() => name[i + 1..].to_lowercase(),
                _ => String::new(),
            }
        };
        let mut size = 0i64;
        let mut mtime = 0i64;
        if let Ok(meta) = std::fs::metadata(path) {
            size = meta.len() as i64;
            mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
        }
        let _ = db.upsert_file_in_scopes(name, path, is_dir, &ext, size, mtime, scopes);
    }

    /// 处理一条 USN 记录。
    fn process(&mut self, db: &Db, frs: u64, parent: u64, name: &str, attrs: u32, reason: u32) {
        let is_dir = attrs & FILE_ATTR_DIR != 0;
        let structural =
            reason & (USN_FILE_CREATE | USN_FILE_DELETE | USN_RENAME_OLD | USN_RENAME_NEW) != 0;

        // 新建 / 改名落位：登记映射 + 写回索引
        if reason & (USN_FILE_CREATE | USN_RENAME_NEW) != 0 {
            self.entries.insert(frs, (parent, name.to_string(), is_dir));
            if !name.starts_with('.') {
                if let Some(path) = self.resolve_from(parent, name) {
                    let scopes = self.scopes_for(&path);
                    if !scopes.is_empty() {
                        self.upsert(db, &path, name, is_dir, &scopes);
                    }
                }
            }
        }
        // 删除 / 改名离位：先拼旧路径再撤映射 + 清索引（目录连子树）
        if reason & (USN_FILE_DELETE | USN_RENAME_OLD) != 0 {
            if let Some(e) = self.entries.remove(&frs) {
                if !e.1.starts_with('.') {
                    if let Some(path) = self.resolve_from(e.0, &e.1) {
                        let _ = db.remove_file(&path);
                        if e.2 {
                            let _ = db.remove_tree(&path);
                        }
                    }
                }
            }
        }
        // 内容/属性写完（CLOSE 且带数据位）且无结构变化：刷新 size/mtime
        if !structural
            && reason & USN_CLOSE != 0
            && reason & (USN_DATA_OVERWRITE | USN_DATA_EXTEND | USN_BASIC_INFO) != 0
        {
            if let Some(e) = self.entries.get(&frs) {
                let (p, n, d) = (e.0, e.1.clone(), e.2);
                if !n.starts_with('.') {
                    if let Some(path) = self.resolve_from(p, &n) {
                        let scopes = self.scopes_for(&path);
                        if !scopes.is_empty() {
                            self.upsert(db, &path, &n, d, &scopes);
                        }
                    }
                }
            }
        }
    }
}

/// 监听一个 NTFS 盘；返回 false 表示启动失败（调用方回退）。
fn watch_drive_usn(
    db: &Db,
    letter: &str,
    drive_scoped: bool,
    common_prefixes: Vec<String>,
) -> bool {
    let Ok(h) = open_volume(letter) else {
        return false;
    };
    // 先查（记 NextUsn）再枚举：枚举期间的变更由从 NextUsn 起的读取补上
    let (mut jid, mut next) = match query_journal(h) {
        Some(v) => v,
        None => {
            create_journal(h);
            match query_journal(h) {
                Some(v) => v,
                None => {
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                    return false;
                }
            }
        }
    };
    let entries = match enum_mft(h) {
        Some(m) => m,
        None => {
            unsafe {
                let _ = CloseHandle(h);
            }
            return false;
        }
    };
    crate::log_msg(&format!(
        "USN 监听 {letter}: 已启动（MFT {} 项，common 前缀 {} 个）",
        entries.len(),
        common_prefixes.len()
    ));
    let root = format!("{letter}:\\");
    let mut w = UsnDrive {
        letter: letter.to_string(),
        root,
        entries,
        drive_scoped,
        common_prefixes,
    };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let rud = READ_USN_JOURNAL_DATA_V1 {
            StartUsn: next,
            ReasonMask: 0xFFFF_FFFF,
            ReturnOnlyOnClose: 0,
            // BytesToWaitFor = 1 → 没有新记录时阻塞等待，平时零 CPU
            Timeout: 0,
            BytesToWaitFor: 1,
            UsnJournalID: jid,
            MinMajorVersion: 2,
            MaxMajorVersion: 3,
        };
        let mut ret = 0u32;
        let r = unsafe {
            DeviceIoControl(
                h,
                FSCTL_READ_USN_JOURNAL,
                Some(&rud as *const _ as *const c_void),
                std::mem::size_of::<READ_USN_JOURNAL_DATA_V1>() as u32,
                Some(buf.as_mut_ptr() as *mut c_void),
                buf.len() as u32,
                Some(&mut ret),
                None,
            )
        };
        if r.is_err() {
            // Journal 可能被删除 / 失效：重建并重枚举后再继续
            std::thread::sleep(Duration::from_secs(3));
            if let Some((id2, u2)) = query_journal(h) {
                if id2 != jid {
                    jid = id2;
                    if let Some(m) = enum_mft(h) {
                        w.entries = m;
                    }
                }
                next = u2;
            } else {
                create_journal(h);
                if let Some((id2, u2)) = query_journal(h) {
                    jid = id2;
                    next = u2;
                    if let Some(m) = enum_mft(h) {
                        w.entries = m;
                    }
                }
            }
            continue;
        }
        let n = ret as usize;
        if n < 8 {
            continue;
        }
        // 输出前 8 字节 = 下一次读取的 USN
        next = i64::from_le_bytes(buf[0..8].try_into().unwrap_or([0u8; 8]));
        let mut off = 8usize;
        while off + 60 <= n {
            let reclen = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap_or([0u8; 4])) as usize;
            if reclen == 0 || off + reclen > n {
                break;
            }
            let rec = &buf[off..off + reclen];
            let major = u16::from_le_bytes(rec[4..6].try_into().unwrap_or([0u8; 2]));
            if major == 2 {
                let frs = u64::from_le_bytes(rec[8..16].try_into().unwrap_or([0u8; 8]));
                let parent = u64::from_le_bytes(rec[16..24].try_into().unwrap_or([0u8; 8]));
                let reason = u32::from_le_bytes(rec[40..44].try_into().unwrap_or([0u8; 4]));
                let attrs = u32::from_le_bytes(rec[52..56].try_into().unwrap_or([0u8; 4]));
                let fnlen = u16::from_le_bytes(rec[56..58].try_into().unwrap_or([0u8; 2])) as usize;
                let fnoff = u16::from_le_bytes(rec[58..60].try_into().unwrap_or([0u8; 2])) as usize;
                if fnoff + fnlen <= reclen {
                    let raw = &rec[fnoff..fnoff + fnlen];
                    let name_w: Vec<u16> = raw
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let name = String::from_utf16_lossy(&name_w);
                    w.process(db, frs, parent, &name, attrs, reason);
                }
            }
            off += reclen;
        }
    }
}

// ==================== 浅层回退（ReadDirectoryChangesW，直接子项） ====================

/// 永久监听一个根目录；句柄失效或溢出后自动重开。
fn watch_root(db: Db, root: PathBuf, scope: String) {
    loop {
        if let Ok(h) = open_dir(&root) {
            let mut buf = vec![0u8; BUF_LEN];
            loop {
                match read_changes(h, &mut buf) {
                    Some(infos) => {
                        for ni in infos {
                            apply(&db, &root, &ni, &scope);
                        }
                    }
                    None => break,
                }
            }
            unsafe {
                let _ = CloseHandle(h);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn open_dir(p: &Path) -> Result<HANDLE, ()> {
    let wide: Vec<u16> = p
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    match h {
        Ok(h) if h == INVALID_HANDLE_VALUE => Err(()),
        Ok(h) => Ok(h),
        Err(_) => Err(()),
    }
}

/// 阻塞读取一次变更；返回 None 表示溢出 / 失败（需重开句柄）。
fn read_changes(h: HANDLE, buf: &mut [u8]) -> Option<Vec<Notify>> {
    let mut bytes = 0u32;
    let ok = unsafe {
        ReadDirectoryChangesW(
            h,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as u32,
            false, // 仅监听直接子项，保证路径精确
            FILE_NOTIFY_CHANGE_FILE_NAME
                | FILE_NOTIFY_CHANGE_DIR_NAME
                | FILE_NOTIFY_CHANGE_SIZE
                | FILE_NOTIFY_CHANGE_LAST_WRITE,
            Some(&mut bytes),
            Some(std::ptr::null_mut()),
            None,
        )
    };
    let success = ok.is_ok();
    if !success || bytes == 0 {
        return None;
    }
    Some(parse(buf, bytes as usize))
}

struct Notify {
    action: u32,
    name: String,
}

/// 解析 FILE_NOTIFY_INFORMATION 链表（每条只含相对根的直接子项名）。
fn parse(buf: &[u8], len: usize) -> Vec<Notify> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + 12 <= len {
        let next =
            u32::from_ne_bytes([buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]])
                as usize;
        let action =
            u32::from_ne_bytes([buf[offset + 4], buf[offset + 5], buf[offset + 6], buf[offset + 7]]);
        let fnlen =
            u32::from_ne_bytes([buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11]])
                as usize;
        let name_start = offset + 12;
        let name_end = name_start + fnlen;
        if name_end > len || fnlen % 2 != 0 {
            break;
        }
        let name_w: Vec<u16> = buf[name_start..name_end]
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .collect();
        out.push(Notify {
            action,
            name: String::from_utf16_lossy(&name_w),
        });
        if next == 0 {
            break;
        }
        offset += next;
    }
    out
}

fn apply(db: &Db, root: &Path, ni: &Notify, scope: &str) {
    let child = root.join(&ni.name);
    let path = child.to_string_lossy().into_owned();
    match ni.action {
        ACTION_REMOVED | ACTION_RENAMED_OLD => {
            // 可能是文件也可能是目录：删自身 + 删子树（幂等，安全）
            let _ = db.remove_file(&path);
            let _ = db.remove_tree(&path);
        }
        ACTION_ADDED | ACTION_RENAMED_NEW | ACTION_MODIFIED => {
            if let Ok(meta) = std::fs::symlink_metadata(&child) {
                if meta.is_dir() {
                    let _ = db.upsert_file(&ni.name, &path, true, "", 0, 0, scope);
                } else {
                    let size = meta.len() as i64;
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let ext = child
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    let _ = db.upsert_file(&ni.name, &path, false, &ext, size, mtime, scope);
                }
            } else {
                let _ = db.remove_file(&path);
                let _ = db.remove_tree(&path);
            }
        }
        _ => {}
    }
}
