use crate::db::Db;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// 桌面 / 分区里的一个文件或文件夹。
/// 字段用 snake_case，序列化时统一转成 camelCase 给前端（is_dir / modified_at）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopFile {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub ext: String,
    pub size: u64,
    pub modified_at: String,
    /// 创建时间（文件进入该文件夹的时刻）。Windows 复制/移动会保留"修改时间"，
    /// 只有"创建时间"能真实反映"刚被加进这个文件夹"，所以按时间排序用它。
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizeResult {
    pub moved: i64,
    pub failed: i64,
    pub details: Vec<OrganizeDetail>,
    /// 每条失败的可读原因（文件被占用 / 权限不足 / 目标同名…）。
    /// 以前只给个数（`failed: 3`），用户不知道哪个文件为什么没动 —— 静默跳过
    /// 换来的就是"我整理了，但东西还在桌面"的困惑。前端取第一条弹提示。
    #[serde(default)]
    pub errors: Vec<String>,
}

impl OrganizeResult {
    pub fn new() -> Self {
        Self { moved: 0, failed: 0, details: Vec::new(), errors: Vec::new() }
    }
}

impl Default for OrganizeResult {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeDetail {
    pub box_name: String,
    pub count: i64,
}

/// 一次「把文件搬进某个分区」的结果。
///
/// 之前落点处理器把所有失败都 `return` 掉了：跨盘 rename 失败、文件被占用、
/// 权限不足……用户看到的就是「拖过去了，什么都没发生」，也没有任何提示。
/// 这里把成败逐条记下来回传前端，前端据此弹提示。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResult {
    /// 移动进来的条目数
    pub moved: i64,
    /// 复制进来的条目数
    pub copied: i64,
    /// 跳过的条目数（拖回自己所在分区 = 调位置，不算搬运）
    pub skipped: i64,
    /// 失败的条目数
    pub failed: i64,
    /// 失败原因（已本地化，可直接展示）
    pub errors: Vec<String>,
}

/// 一条已启用的整理规则（仅 crate 内部消费，所以不需要 pub）
#[derive(Debug, Clone)]
pub(crate) struct RuleRow {
    id: i64,
    box_id: i64,
    match_type: String,
    pattern: String,
}

const WINDOWS_HIDDEN: u32 = 0x2;
const WINDOWS_SYSTEM: u32 = 0x4;

pub fn desktop_dir() -> Result<PathBuf, String> {
    dirs::desktop_dir().ok_or_else(|| "无法获取桌面路径".to_string())
}

fn file_attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

fn is_hidden_or_system(meta: &fs::Metadata) -> bool {
    let attrs = file_attributes(meta);
    (attrs & WINDOWS_HIDDEN) != 0 || (attrs & WINDOWS_SYSTEM) != 0
}

pub fn load_enabled_rules(db: &Db) -> Result<Vec<RuleRow>, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT r.id, r.box_id, r.match_type, r.pattern
             FROM rules r WHERE r.enabled = 1
             ORDER BY r.priority ASC, r.id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RuleRow {
                id: r.get(0)?,
                box_id: r.get(1)?,
                match_type: r.get(2)?,
                pattern: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut rules = Vec::new();
    for row in rows {
        rules.push(row.map_err(|e| e.to_string())?);
    }
    Ok(rules)
}

pub fn scan_desktop(exclude_dirs: &[String], include_shortcuts: bool) -> Result<Vec<DesktopFile>, String> {
    let dir = desktop_dir()?;
    let mut files = Vec::new();
    let entries = fs::read_dir(&dir).map_err(|e| format!("读取桌面失败: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if lower == "desktop.ini" || lower == "$recycle.bin" || lower == "system volume information" {
            continue;
        }
        if exclude_dirs.iter().any(|d| d.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if is_hidden_or_system(&meta) {
            continue;
        }
        if meta.is_dir() {
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !include_shortcuts && ext == "lnk" {
            continue;
        }
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        files.push(DesktopFile {
            name,
            path: path.to_string_lossy().to_string(),
            is_dir: false,
            ext,
            size: meta.len(),
            modified_at: modified,
            created_at: String::new(),
        });
    }
    Ok(files)
}

fn parse_size_token(token: &str) -> Option<(char, u64)> {
    let token = token.trim();
    let (op, rest) = if let Some(r) = token.strip_prefix('>') {
        ('>', r)
    } else if let Some(r) = token.strip_prefix('<') {
        ('<', r)
    } else {
        ('=', token)
    };
    let rest = rest.trim().to_uppercase();
    let (num_part, unit): (String, u64) = if let Some(n) = rest.strip_suffix("GB") {
        (n.to_string(), 1024 * 1024 * 1024)
    } else if let Some(n) = rest.strip_suffix("MB") {
        (n.to_string(), 1024 * 1024)
    } else if let Some(n) = rest.strip_suffix("KB") {
        (n.to_string(), 1024)
    } else if let Some(n) = rest.strip_suffix('B') {
        (n.to_string(), 1)
    } else {
        (rest.clone(), 1)
    };
    let num: f64 = num_part.trim().parse().ok()?;
    Some((op, (num * unit as f64) as u64))
}

fn match_size(file_size: u64, pattern: &str) -> bool {
    parse_size_token(pattern).map_or(false, |(op, bound)| match op {
        '>' => file_size > bound,
        '<' => file_size < bound,
        _ => file_size == bound,
    })
}

fn match_date(modified_at: &str, pattern: &str) -> bool {
    let Ok(days) = pattern.trim().trim_start_matches(['>', '<']).parse::<i64>() else {
        return false;
    };
    let Ok(mtime) = chrono::NaiveDateTime::parse_from_str(modified_at, "%Y-%m-%d %H:%M:%S") else {
        return false;
    };
    let age_days = (chrono::Local::now().naive_local() - mtime).num_days();
    if pattern.trim().starts_with('<') {
        age_days < days
    } else {
        age_days > days
    }
}

pub fn match_file(file: &DesktopFile, rules: &[RuleRow], skip_rule_id: Option<i64>) -> Option<i64> {
    match_file_detailed(file, rules, skip_rule_id).map(|(id, _)| id)
}

/// 规则命中的**人话**依据（预览列表里给用户看："为什么这个文件要去 A 分区"）。
fn rule_match_note(rule: &RuleRow) -> String {
    match rule.match_type.as_str() {
        "extension" => format!("扩展名 {}", rule.pattern),
        "keyword" => format!("关键词 {}", rule.pattern),
        "size" => format!("大小 {}", rule.pattern),
        "date" => format!("时间 {}", rule.pattern),
        _ => rule.match_type.clone(),
    }
}

/// 同 [`match_file`]，但把命中的**规则依据**一起带回来（预览用）。
pub fn match_file_detailed(
    file: &DesktopFile,
    rules: &[RuleRow],
    skip_rule_id: Option<i64>,
) -> Option<(i64, String)> {
    for rule in rules {
        if Some(rule.id) == skip_rule_id {
            continue;
        }
        let hit = match rule.match_type.as_str() {
            "extension" => {
                if file.ext.is_empty() {
                    false
                } else {
                    rule.pattern
                        .split(',')
                        .map(|p| p.trim().to_lowercase())
                        .any(|p| !p.is_empty() && p == file.ext)
                }
            }
            "keyword" => {
                let kw = rule.pattern.trim().to_lowercase();
                !kw.is_empty() && file.name.to_lowercase().contains(&kw)
            }
            "size" => match_size(file.size, &rule.pattern),
            "date" => match_date(&file.modified_at, &rule.pattern),
            _ => false,
        };
        if hit {
            return Some((rule.box_id, rule_match_note(rule)));
        }
    }
    None
}

/// 告诉资源管理器"这个路径的属性变了，重新读一遍"。
///
/// 只调 `SetFileAttributesW` 是**不够**的：桌面/资源管理器窗口把每一项的属性
/// 缓存在自己那侧，不会因为属性被别的进程改了就去重画。表现就是我们这边明明已经
/// 把分区文件夹的隐藏位清掉了，桌面上还是看不见那个文件夹，**按一下 F5 才冒出来** ——
/// 用户看到的就是"解散分区之后文件夹被隐藏了、进不去"。SHChangeNotify 是官方通知方式。
///
/// `SHCNF_FLUSHNOWAIT`：不等资源管理器处理完就返回，不拖慢搬运/解散流程。
fn notify_attrs_changed(path: &Path) {
    use windows::core::HSTRING;
    use windows::Win32::UI::Shell::{
        SHChangeNotify, SHCNE_ATTRIBUTES, SHCNF_FLUSHNOWAIT, SHCNF_PATHW,
    };
    let s = HSTRING::from(path.as_os_str());
    unsafe {
        SHChangeNotify(
            SHCNE_ATTRIBUTES,
            SHCNF_PATHW | SHCNF_FLUSHNOWAIT,
            Some(s.as_ptr() as *const core::ffi::c_void),
            None,
        );
    }
}

/// 这个路径现在是不是带 HIDDEN / SYSTEM（读不到就算 false）。
fn path_is_hidden(path: &Path) -> bool {
    matches!(fs::metadata(path), Ok(m) if is_hidden_or_system(&m))
}

/// 把文件夹设为隐藏+系统属性，避免在桌面上显示为普通文件夹图标。
///
/// **私有**：整个程序里只有"确认这个目录是我们自己建的"之后才允许调用。
/// 对外暴露的话，任何一处按分区名拼路径的代码都可能把用户自己的文件夹打成隐藏。
///
/// 返回是否**确实**隐藏了（写入成功 + 回读确认）。以前是不看返回值、也不回读，
/// 写失败时上层完全不知道，于是"该隐藏的没隐藏、该显示的还是看不见"。
fn hide_dir(path: &Path) -> bool {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM,
    };
    let s = HSTRING::from(path.as_os_str());
    let wrote = unsafe {
        SetFileAttributesW(PCWSTR(s.as_ptr()), FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM).is_ok()
    };
    notify_attrs_changed(path);
    let ok = wrote && path_is_hidden(path);
    if !ok {
        crate::log_msg(&format!("设置隐藏属性失败：{}", path.display()));
    }
    ok
}

/// 取消隐藏：清掉 HIDDEN|SYSTEM 两个位，其余属性（只读 / 存档 / 索引）原样保留。
/// 解散分区时用它把分区文件夹**交还给用户**，桌面上能直接看到、能进去。
///
/// 写入后回读确认，并通知资源管理器刷新（见 [`notify_attrs_changed`]）——
/// 少了这一步，用户看到的就是"分区解散了，可文件夹还是不见，得按 F5"。
///
/// 返回是否**确实**不再隐藏。
pub fn unhide_dir(path: &Path) -> bool {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let cleared = file_attributes(&meta) & !(WINDOWS_HIDDEN | WINDOWS_SYSTEM);
    let s = HSTRING::from(path.as_os_str());
    let wrote = unsafe {
        SetFileAttributesW(PCWSTR(s.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(cleared)).is_ok()
    };
    notify_attrs_changed(path);
    let ok = wrote && !path_is_hidden(path);
    if !ok {
        crate::log_msg(&format!("取消隐藏失败：{}", path.display()));
    }
    ok
}

/// 这个目录是不是**我们自己的**分区文件夹。
///
/// 分区文件夹由本程序创建并主动打上 HIDDEN|SYSTEM。这个判断是所有会改动目录的
/// 操作（改属性 / 搬空 / 删除 / 重命名）的**唯一闸门**：只有返回 true 才允许碰它。
///
/// 用户在桌面上自己的同名文件夹不带宽这两个属性 → 永远返回 false →
/// 我们一根手指都不碰（不隐藏、不搬、不删、不改名）。这是"绝不碰用户文件"的底线。
pub fn is_managed_box_dir(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) && path_is_hidden(path)
}

/// 重新给"自己的"分区文件夹刷一遍隐藏属性（Windows 有时会把属性重置掉）。
/// 目录不存在、或不是我们建的 → 什么都不做并返回 false。
pub fn refresh_box_dir_hidden(path: &Path) -> bool {
    if !is_managed_box_dir(path) {
        return false;
    }
    hide_dir(path)
}

/// 分区落点文件夹。**已存在的同名普通目录一律拒绝**：
/// 那是用户自己的文件夹，不能改属性、不能当分区文件夹用。
pub fn ensure_box_dir(name: &str) -> Result<PathBuf, String> {
    let dir = desktop_dir()?.join(name);
    if dir.exists() {
        if !is_managed_box_dir(&dir) {
            return Err(format!(
                "桌面上已存在同名文件夹「{name}」，它不是本程序创建的分区文件夹，不会对它做任何改动"
            ));
        }
        return Ok(dir);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("创建分区文件夹失败: {e}"))?;
    // 只有确定是新目录才打隐藏：分区文件夹对桌面隐藏，只显示透明面板
    hide_dir(&dir);
    Ok(dir)
}

/// 找出 `names` 里形如 `<prefix> N` 的最大序号 N（没有就 0）。
fn max_name_suffix(names: &[String], prefix: &str) -> i64 {
    let head = format!("{prefix} ");
    names
        .iter()
        .filter_map(|n| n.strip_prefix(&head)?.trim().parse::<i64>().ok())
        .max()
        .unwrap_or(0)
}

/// 分区名是否可用：数据库里没有重名；`need_desktop_dir` 为真时，桌面上也不能有同名目录。
pub fn box_name_available(db: &Db, name: &str, need_desktop_dir: bool) -> Result<bool, String> {
    if db.box_names()?.iter().any(|n| n == name) {
        return Ok(false);
    }
    if need_desktop_dir && desktop_dir()?.join(name).exists() {
        return Ok(false);
    }
    Ok(true)
}

/// 取一个**一定可用**的默认分区名：`<prefix> N`，N 从"已有最大序号 + 1"开始往上试。
///
/// 关键是**从最大序号取号**，而不是按"现有分区个数 + 1"：分区会被解散、会被改名，
/// 个数和最大序号很快就不相等。以前前端按个数取号，7 个"文件夹 N"分区算出下一个是
/// "文件夹 8"，可数据库里恰好已经有个"文件夹 8" —— 于是"新建分区"永远失败。
///
/// 另外还要跳过桌面上已存在的同名目录：普通分区要在那里建自己的文件夹，
/// 撞上用户的真实文件夹就只能换号（我们自己建的隐藏分区文件夹也算 "已存在"，
/// 但那是受管目录，这里一并跳过更省事，反正换号不影响用户）。
pub fn pick_free_box_name(db: &Db, prefix: &str, need_desktop_dir: bool) -> Result<String, String> {
    let names = db.box_names()?;
    let desktop = desktop_dir()?;
    let mut n = max_name_suffix(&names, prefix) + 1;
    for _ in 0..1000 {
        let candidate = format!("{prefix} {n}");
        let taken_in_db = names.iter().any(|x| x == &candidate);
        // 关联文件夹分区只在面板上显示这个名字，桌面有没有同名目录都无所谓
        let taken_on_desktop = need_desktop_dir && desktop.join(&candidate).exists();
        if !taken_in_db && !taken_on_desktop {
            return Ok(candidate);
        }
        n += 1;
    }
    Err(format!("无法为「{prefix}」生成一个不重名的分区名"))
}

/// 在一个**完整名字**附近取可用名：`base`、`base 2`、`base 3`…
///
/// 和 [`pick_free_box_name`] 的区别是**整名递增、不拆前缀**。用于关联文件夹分区：
/// 它的默认名来自真实文件夹名，而文件夹名里带数字太平常了（"照片2024"、"v1.2 备份"），
/// 拆前缀会先 `take_while(!is_ascii_digit)` 把数字全切掉，名字就变得面目全非。
pub fn pick_free_box_name_same(db: &Db, base: &str) -> Result<String, String> {
    let names = db.box_names()?;
    if !names.iter().any(|n| n == base) {
        return Ok(base.to_string());
    }
    for i in 2..1000 {
        let candidate = format!("{base} {i}");
        if !names.iter().any(|n| n == &candidate) {
            return Ok(candidate);
        }
    }
    Err(format!("无法为「{base}」生成一个不重名的分区名"))
}

/// 从路径里取出最后一级目录名，作为关联文件夹分区的默认名字。
///
/// `I:\资料\项目资料` → `项目资料`；路径是盘符根（`I:\`）或取不到名字时返回 `None`，
/// 由调用方退回到普通的自动命名。
pub fn folder_display_name(path: &str) -> Option<String> {
    let p = Path::new(path.trim().trim_end_matches(['\\', '/']));
    let name = p.file_name()?.to_string_lossy().trim().to_string();
    // "I:" 这种盘符根取出来不是目录名，别拿它当分区名
    if name.is_empty() || name.ends_with(':') {
        None
    } else {
        Some(name)
    }
}

pub fn unique_target(dir: &Path, file_name: &str) -> PathBuf {
    unique_target_kind(dir, file_name, false)
}

/// 同 [`unique_target`]，但知道目标是不是文件夹。
///
/// 文件夹名里带点是常态（"v1.2 备份"），按 `file_stem`/`extension` 拆会把名字拆坏，
/// 生成出 "v1.2 备份 (1)" 之外还会丢掉 ".2 备份" 这一段。所以文件夹整名重名计数。
pub fn unique_target_kind(dir: &Path, file_name: &str, is_dir: bool) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = if is_dir {
        (file_name.to_string(), String::new())
    } else {
        (
            Path::new(file_name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| file_name.to_string()),
            Path::new(file_name)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
        )
    };
    for i in 1..1000 {
        let name = format!("{stem} ({i}){ext}");
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}_{}", chrono::Local::now().timestamp_millis()) + &ext)
}

/// `fs::rename` 跨盘会直接失败（ERROR_NOT_SAME_DEVICE = 17）。
/// 关联文件夹分区指向的目录经常在别的盘（D:/E:/U 盘），不认这个错就会表现为
/// 「从这个分区往那个分区拖，一点反应都没有」。
fn is_cross_device(err: &std::io::Error) -> bool {
    const ERROR_NOT_SAME_DEVICE: i32 = 17;
    err.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE)
}

/// 递归复制目录（用于跨盘搬运文件夹 / 复制文件夹）
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    // 目标落在源里面会无限递归，直接拒绝
    if dst.starts_with(src) {
        return Err(format!("不能把「{}」放进它自己里面", src.display()));
    }
    fs::create_dir_all(dst).map_err(|e| format!("创建 {} 失败: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("读取 {} 失败: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        // 符号链接 / 目录连接（junction）：跟着走可能绕成环（指向自己的祖先目录），
        // 与其挂死不如明确报错，让用户先处理掉那个链接。
        if ft.is_symlink() {
            return Err(format!(
                "「{}」里有符号链接或目录连接，暂不支持整体搬运",
                src.display()
            ));
        }
        if ft.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)
                .map_err(|e| format!("复制 {} 失败: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// 把 `src`（文件或文件夹）搬 / 复制到目录 `to_dir` 下，返回落点路径。
///
/// - 同盘：`fs::rename`，瞬间完成；
/// - 跨盘：rename 必然失败 → 交给系统自己搬（见 `move_across_volumes`）；
/// - `copy = true`：只复制，源不动。
pub fn transfer(src: &Path, to_dir: &Path, copy: bool) -> Result<PathBuf, String> {
    if !src.exists() {
        return Err(format!("{} 不存在", src.display()));
    }
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return Err(format!("无法搬运 {}", src.display()));
    }
    let is_dir = src.is_dir();
    let dst = unique_target_kind(to_dir, &name, is_dir);

    if copy {
        if is_dir {
            copy_dir_all(src, &dst)?;
        } else {
            fs::copy(src, &dst).map_err(|e| format!("复制 {name} 失败: {e}"))?;
        }
        return Ok(dst);
    }

    match fs::rename(src, &dst) {
        Ok(_) => Ok(dst),
        Err(e) if is_cross_device(&e) => {
            // 跨盘：交给系统搬（落点万一被占会改名，所以以返回值为准）
            let final_dst = move_across_volumes(src, &dst, is_dir)?;
            Ok(final_dst)
        }
        Err(e) => Err(format!("移动 {name} 失败: {e}")),
    }
}

/// 跨盘（不同卷）搬运：搬完返回**实际落点**（可能因为重名而改了名）。
///
/// 以前这里是"自己 `fs::copy` → 再 `fs::remove_file`"，删源那步时不时报
/// `系统找不到指定的文件 (os error 2)`：文件其实早复制到位了，只是源在这一瞬间
/// 被系统 / 别的程序动了，于是给用户弹一条"已复制到目标分区，但原文件删除失败，
/// 请手动删除"的**假错误** —— 文件好好的，却让人以为出事了。
///
/// 现在交给 `MoveFileExW(MOVEFILE_COPY_ALLOWED)`：**跨卷移动由系统一步做完**，
/// 根本不存在"复制完再删源"这个中间态（资源管理器自己就是这么搬的）。
///
/// 它不带替换语义，所以落点被占（并发搬运 / 落点重算撞车，实测会报
/// `文件存在 0x80070050`）时会失败 —— 这时换个不重名的落点再试一次，
/// 而不是去覆盖一个已经在那儿的文件。只有它彻底走不通才退回手动复制，
/// 并且只在「源确实还在、确实删不掉」时才算失败。
fn move_across_volumes(src: &Path, dst: &Path, is_dir: bool) -> Result<PathBuf, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_COPY_ALLOWED};

    /// 让系统搬一次；`MOVEFILE_COPY_ALLOWED` = 跨卷也搬，不覆盖已存在的落点。
    unsafe fn sys_move(src: &Path, dst: &Path) -> windows::core::Result<()> {
        let s = HSTRING::from(src.as_os_str());
        let d = HSTRING::from(dst.as_os_str());
        MoveFileExW(PCWSTR(s.as_ptr()), PCWSTR(d.as_ptr()), MOVEFILE_COPY_ALLOWED)
    }

    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if unsafe { sys_move(src, dst) }.is_ok() {
        return Ok(dst.to_path_buf());
    }
    // 系统搬失败，但"目标已在、源已不在"——那就是搬成了，只是这一步报错而已
    if dst.exists() && !src.exists() {
        crate::log_msg(&format!(
            "transfer: 「{name}」跨盘移动报错，但目标已就位且源已不在，按成功处理"
        ));
        return Ok(dst.to_path_buf());
    }

    // 落点被占了 → 换个不重名的再试一次。只有源还在（= 确实没搬动）才重试，
    // 免得同一个文件被搬两遍。**绝不覆盖**已经存在的那个文件。
    let mut target = dst.to_path_buf();
    if src.exists() {
        if let Some(dir) = dst.parent() {
            let fresh = unique_target_kind(dir, &name, is_dir);
            if fresh != dst {
                crate::log_msg(&format!(
                    "transfer: 「{name}」落点被占，改用 {} 重试",
                    fresh.display()
                ));
                if unsafe { sys_move(src, &fresh) }.is_ok() || (fresh.exists() && !src.exists()) {
                    return Ok(fresh);
                }
            }
        }
        // 还没成：自己也别去覆盖原落点，重新挑个空位
        if let Some(dir) = dst.parent() {
            target = unique_target_kind(dir, &name, is_dir);
        }
    }

    // 兜底：自己复制（源不存在的话这里会如实报"不存在"）
    if is_dir {
        copy_dir_all(src, &target)?;
    } else {
        fs::copy(src, &target).map_err(|e| format!("复制 {name} 失败: {e}"))?;
    }
    let removed = if is_dir {
        fs::remove_dir_all(src)
    } else {
        fs::remove_file(src)
    };
    match removed {
        Ok(_) => Ok(target),
        Err(re) => {
            // 源已经不在 = 搬成了（复制的同时源就被清掉了），别再吓唬用户
            if !src.exists() {
                crate::log_msg(&format!(
                    "transfer: 「{name}」删源报 {re}，但源已不存在，按成功处理"
                ));
                return Ok(target);
            }
            Err(format!(
                "「{name}」已复制到目标分区，但原文件删除失败（{re}），请手动删除"
            ))
        }
    }
}

/// 记录一次搬运，并**保留最初的原始位置**。
///
/// 关键：分区 A → 分区 B 之后，如果新插一条 `from = 分区A 的隐藏文件夹`，
/// 那么删掉分区 B 时文件会被"退回"到 `<桌面>\分区A\` 这个隐藏目录里 —— 用户既看不到
/// 也进不去。所以这里先看历史里有没有「`to_path` == 这文件的当前路径」的记录，
/// 有就把它续上（`from` 保持最初的桌面位置，`to` 改成新落点）。
fn record_transfer(db: &Db, name: &str, from: &Path, to: &Path) {
    let from_s = from.to_string_lossy().to_string();
    let to_s = to.to_string_lossy().to_string();
    if let Ok(Some((id, origin))) = db.move_history_origin(&from_s) {
        let _ = db.update_move_history(id, &origin, &to_s, name);
        return;
    }
    db.record_move(name, &from_s, &to_s);
}

/// 让资源管理器立即刷新某个目录的所有视图。
///
/// 我们把文件移进分区（普通分区 = `桌面\<分区名>` 隐藏文件夹）后，资源管理器里
/// **源目录的视图不会马上重读**，残留一个"幽灵条目"—— 用户一点就弹
/// 「找不到该项目」，看起来像文件丢了；再加上开着的"显示隐藏文件"能看到
/// 分区文件夹就在桌面上，整体观感就成了"拖进软件 → 提示找不到 → 文件跑到桌面"。
/// 广播一次 shell 变更通知，所有打开着这个目录的窗口马上刷新，幽灵条目消失。
#[cfg(target_os = "windows")]
fn refresh_shell_views(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_PATHW};
    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, Some(wide.as_ptr().cast()), None);
    }
}

pub fn move_file(db: &Db, from: &Path, to_dir: &Path) -> Result<PathBuf, String> {
    // 已经在目标文件夹里（最典型的是"把分区里的文件又拖回同一个分区"）→ 直接返回。
    // 不拦的话会被当成同名冲突，交给 unique_target 改成 "xxx (1)"，
    // 用户会看到文件名莫名其妙多了个后缀。
    if from.parent() == Some(to_dir) {
        return Ok(from.to_path_buf());
    }
    let name = from
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let to = transfer(from, to_dir, false)?;
    record_transfer(db, &name, from, &to);
    // 源目录的 Explorer 视图立刻刷新，别留点了会弹「找不到该项目」的幽灵条目
    #[cfg(target_os = "windows")]
    if let Some(parent) = from.parent() {
        refresh_shell_views(parent);
    }
    Ok(to)
}

/// 把文件 / 文件夹复制到目录 `to_dir` 下，源文件保持不动。
///
/// 复制进来的条目**不写移动历史**：它本来就该跟着分区走（删分区时按"未记录"退回桌面），
/// 记上历史反而会和原件抢同一个"原始位置"。
pub fn copy_file(from: &Path, to_dir: &Path) -> Result<PathBuf, String> {
    if from.parent() == Some(to_dir) {
        return Err("源文件和目标位置是同一个文件夹".to_string());
    }
    transfer(from, to_dir, true)
}

/// 某个分区的落点文件夹：
/// - `folder` 类型（关联文件夹）→ 它链接的那个真实目录；
/// - 其他类型 → 桌面下与该分区同名的隐藏文件夹。
pub fn box_dir_for(db: &Db, box_id: i64) -> Result<PathBuf, String> {
    let (box_type, link, name) = load_box_meta(db, box_id)?;
    if box_type == "folder" {
        match link.filter(|p| !p.trim().is_empty()) {
            Some(p) => Ok(PathBuf::from(p)),
            None => Err("该分区还没有关联文件夹".to_string()),
        }
    } else {
        ensure_box_dir(&name)
    }
}

/// 分区的 (类型, 链接路径, 名称)。普通分区不需要去读 link_path。
fn load_box_meta(db: &Db, box_id: i64) -> Result<(String, Option<String>, String), String> {
    let (_, name, box_type) = load_boxes_with_id(db)?
        .into_iter()
        .find(|(id, _, _)| *id == box_id)
        .ok_or_else(|| "分区不存在".to_string())?;
    if box_type != "folder" {
        return Ok((box_type, None, name));
    }
    let link = db.box_type_info(box_id)?.and_then(|(_, link, _, _)| link);
    Ok((box_type, link, name))
}

/// 同一次拖拽会被**两路同时**处理：分区窗口的落点事件，加上 `start_file_drag`
/// 里"按光标位置兜底"的那一路。两路都去搬同一个文件 → 先到的一路搬走，
/// 后到的一路找不到源，于是给用户弹一条"复制 xxx 失败：系统找不到指定的文件"的
/// 假错误（文件明明已经好好地躺在新分区里）。
///
/// 这里按**源路径**去重：同一路径 3 秒内只认领一次，重复的那一路直接跳过。
/// 3 秒足够覆盖"两路几乎同时进来"的情况，又不会挡住用户真的再拖一次。
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 认领一次搬运。`false` = 已经有别的一路在搬 / 刚搬过，这次应当跳过。
///
/// 顺带清掉过期条目，免得这张表一直长大。
fn claim_transfer(path: &str) -> bool {
    let mut m = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    m.retain(|_, t| t.elapsed() < Duration::from_secs(3));
    let key = path.to_lowercase();
    if let Some(t) = m.get(&key) {
        if t.elapsed() < Duration::from_secs(3) {
            return false;
        }
    }
    m.insert(key, Instant::now());
    true
}

/// 把一批路径搬 / 复制进某个分区。落点处理器与兜底路径共用这一份实现。
pub fn transfer_into_box(
    db: &Db,
    box_id: i64,
    paths: &[String],
    copy: bool,
) -> Result<TransferResult, String> {
    let dir = box_dir_for(db, box_id)?;
    let mut r = TransferResult::default();
    for p in paths {
        let src = Path::new(p);
        if src.parent() == Some(dir.as_path()) {
            // 拖回自己这一格 = 调整图标位置，不是搬运（位置由前端处理）
            r.skipped += 1;
            continue;
        }
        if !src.exists() {
            // 源已经不在了 —— 同一次拖拽里已经有一路把它搬走了。
            // 这是"重复的一路"，不是失败：文件好好的，只是不该再搬一遍。
            // 以前这里算 failed，用户就会看到"复制 xxx 失败：找不到文件"的假错误。
            crate::log_msg(&format!("transfer: 源已不在，跳过（应已被另一路搬走）：{p}"));
            r.skipped += 1;
            continue;
        }
        if !claim_transfer(p) {
            // 同一个源路径 3 秒内第二次进来 —— 重复的一路，跳过（见 IN_FLIGHT 说明）
            crate::log_msg(&format!("transfer: 同一路径正在搬运中，跳过重复的一路：{p}"));
            r.skipped += 1;
            continue;
        }
        let res = if copy {
            copy_file(src, &dir)
        } else {
            move_file(db, src, &dir)
        };
        match res {
            Ok(_) => {
                if copy {
                    r.copied += 1;
                } else {
                    r.moved += 1;
                }
            }
            Err(e) => {
                // 搬运结果只有前端一条 toast，事后排查全靠猜 —— 这里把
                // 源、目标、失败原因落到日志，下次复现能直接看明白
                crate::log_msg(&format!(
                    "transfer: {} 失败 {p} → {dir} ：{e}",
                    if copy { "复制" } else { "移动" },
                    dir = dir.display()
                ));
                r.failed += 1;
                r.errors.push(e);
            }
        }
    }
    Ok(r)
}

/// 可以被我们安全接管的 `<桌面>\<分区名>` 目录集合。
///
/// 判定规则（这是"退出归还 / 启动收回"两条自动搬运链路的准入名单）：
/// - 目录已存在 → 必须带 HIDDEN|SYSTEM（= 我们建的）才算数；
/// - 目录不存在 → 也算数（新建一个空目录不会碰到任何用户数据），收回文件时按需补建。
///
/// **快捷访问分区（`folder` 类型）永远不在名单里** —— 它链接的是用户自己的
/// 真实目录，程序不对它做任何移动 / 隐藏。
fn owned_box_dirs(db: &Db, desktop: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs = Vec::new();
    for (_id, name, box_type) in load_boxes_with_id(db)? {
        if box_type == "folder" {
            continue;
        }
        let dir = desktop.join(&name);
        // 同名但不是我建的（用户自己的文件夹）→ 不纳入名单，绝不碰
        if dir.exists() && !is_managed_box_dir(&dir) {
            continue;
        }
        dirs.push(dir);
    }
    Ok(dirs)
}

/// 路径是不是正好落在名单里的某个分区文件夹里（只看直接父目录）。
fn inside_owned(dir: &Path, owned: &[PathBuf]) -> bool {
    owned.iter().any(|d| dir.parent() == Some(d.as_path()))
}

/// 记录里的"原始位置"能不能当真？（能不能拿它当归还目标）
///
/// 不能当真的两种情况：
/// - 它就是文件自己（`from == to`，历史脏数据）；
/// - 它落在**别的分区文件夹**里。老版本把"分区 A 的文件拖进分区 B"记成了
///   `from = A 的隐藏目录`，解散 B 时按这条记录归还，文件就被搬进 A 那个
///   用户看不见也进不去的隐藏文件夹 —— 这正是"解散之后文件不见了 / 没回到原位"的来源。
fn origin_usable(from: &str, src: &Path, owned: &[PathBuf]) -> bool {
    if from.trim().is_empty() {
        return false;
    }
    let p = Path::new(from);
    p != src && !inside_owned(p, owned)
}

/// 算出"解散 / 退出归还"时，这个文件应该回到哪里。
///
/// - 记录的原始位置可用 → 回那儿（那儿被占了就在原目录下加序号）；
/// - 不可用（历史脏数据）→ 回**桌面根目录**。分区面板本来就摆在桌面上，
///   用户一眼就能看见，总好过被塞回另一个隐藏的分区文件夹。
pub fn return_target_for(db: &Db, desktop: &Path, src: &Path, from: &str) -> PathBuf {
    let fname = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_dir = src.is_dir();
    let owned = owned_box_dirs(db, desktop).unwrap_or_default();
    if origin_usable(from, src, &owned) {
        let from_path = Path::new(from);
        if !from_path.exists() {
            return from_path.to_path_buf();
        }
        // 原始位置又被别的文件占了 → 放到原目录下、名字加序号
        let parent = from_path.parent().unwrap_or(desktop);
        return unique_target_kind(parent, &fname, is_dir);
    }
    unique_target_kind(desktop, &fname, is_dir)
}

/// 修正历史遗留的错误来源（`from_path`）。
///
/// 只动**落点在我们自己分区文件夹里**的记录，把那些指向分区隐藏目录（或指向自己）
/// 的来源改成桌面上的对应位置。这样"解散归还 / 启动收回 / 退出归还"三条链路
/// 都会把文件送到明处，而不是在隐藏目录之间来回倒。
///
/// 返回修正的条数。
pub fn sanitize_move_history(db: &Db) -> Result<u32, String> {
    let desktop = desktop_dir()?;
    let owned = owned_box_dirs(db, &desktop)?;
    let mut fixed = 0u32;
    for (id, fname, from, to) in db.get_move_history()? {
        let src = Path::new(&to);
        // 落点不在我们自己的分区文件夹里 → 不归我们管，一律不碰
        if !inside_owned(src, &owned) {
            continue;
        }
        if origin_usable(&from, src, &owned) {
            continue;
        }
        let fallback = desktop.join(&fname).to_string_lossy().to_string();
        if fallback == from {
            continue;
        }
        if db.set_move_origin(id, &fallback).is_ok() {
            fixed += 1;
        }
    }
    if fixed > 0 {
        crate::log_msg(&format!(
            "修正 {fixed} 条搬运记录的原始位置（原先指向隐藏的分区文件夹，归还时会掉进看不见的目录）"
        ));
    }
    Ok(fixed)
}

/// 退出时把所有分区里的文件**归还到桌面**（能回到整理前的原始位置就回原位）。
///
/// **故意不删移动记录** —— 下次启动要靠这些记录把文件收回原来的分区
/// （见 [`reorganize_from_history`]）。也不删分区、不动配置，下次打开还是原样。
///
/// **只处理落点在自己分区文件夹里的文件**：拖进"关联文件夹（快捷访问）"的文件
/// 已经到了用户自己的目录里，不归程序管，绝不能每次退出都把它搬出来。
///
/// 重复调用是安全的：文件已经不在分区里时什么都不做。
/// 返回归还的文件数。
pub fn return_all_to_desktop(db: &Db) -> Result<u32, String> {
    let desktop = desktop_dir()?;
    let owned = owned_box_dirs(db, &desktop)?;
    let mut moved = 0u32;

    // 1) 有记录的：回到整理前的原始位置
    for (_id, _name, from, to) in db.get_move_history()? {
        let src = Path::new(&to);
        if !src.exists() {
            continue;
        }
        // 不在自己的分区文件夹里（关联文件夹 / 用户自己挪去别处）→ 不碰
        if !inside_owned(src, &owned) {
            continue;
        }
        // 归还目标统一由 return_target_for 决定：原始位置不可用时回桌面根目录，
        // 绝不能是另一个分区的隐藏文件夹。
        let target = return_target_for(db, &desktop, src, &from);
        if transfer_to_exact(src, &target).is_ok() {
            moved += 1;
        }
    }

    // 2) 没有记录的（历史被清过、或用户直接往分区文件夹里塞的）→ 退到桌面，
    //    同时补一条记录，这样下次启动还能收回同一个分区。
    //    只扫名单内的目录，且目录里还剩东西（被占用）时**不强删**。
    for dir in &owned {
        if !dir.is_dir() {
            continue;
        }
        // 先收集再搬：一边读目录一边往外移，枚举有漏项的风险
        let items: Vec<(String, PathBuf, bool)> = fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter_map(|entry| {
                let p = entry.path();
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.eq_ignore_ascii_case("desktop.ini") {
                    return None;
                }
                let is_dir = p.is_dir();
                Some((fname, p, is_dir))
            })
            .collect();
        for (fname, p, is_dir) in items {
            // 文件夹也要一起退回去：以前直接跳过目录，删分区后它会永远留在
            // 隐藏的分区文件夹里，既看不见也删不掉（remove_dir 还会因非空失败）
            let target = unique_target_kind(&desktop, &fname, is_dir);
            if transfer_to_exact(&p, &target).is_ok() {
                db.record_move(&fname, &target.to_string_lossy(), &p.to_string_lossy());
                moved += 1;
            }
        }
        // 空了才删；没退干净就留着（下次启动还能接着收），绝不留一个用户看不见的
        // 隐藏空壳：删不掉就取消隐藏。
        if fs::remove_dir(dir).is_err() && !unhide_dir(dir) {
            crate::log_msg(&format!(
                "分区文件夹既没删掉也没能恢复显示：{}（下次启动会再试一次）",
                dir.display()
            ));
        }
    }
    Ok(moved)
}

/// 启动时按移动记录把文件**重新收进各分区**。
///
/// 和 [`return_all_to_desktop`] 是一对：退出归还 → 启动收回。
/// 上次如果是崩溃退出（文件还在分区里），这里自然什么都不做（目标已存在），
/// 所以不会重复搬动。
///
/// **只收回落点在自己分区文件夹里的记录**。以前这里对任意记录的落点父目录
/// 都 `create_dir_all` + `hide_dir`，只要历史里有一条落点在关联文件夹里
/// （把桌面文件拖进"快捷访问"分区就是这个效果），启动时就会把用户**自己的
/// 那个文件夹打成隐藏+系统**，桌面上直接消失 —— 这是"文件夹被隐藏"的根源。
pub fn reorganize_from_history(db: &Db) -> Result<u32, String> {
    let desktop = desktop_dir()?;
    let owned = owned_box_dirs(db, &desktop)?;
    let mut moved = 0u32;
    for (id, _name, from, to) in db.get_move_history()? {
        let src = Path::new(&from);
        let dst = Path::new(&to);
        if !inside_owned(dst, &owned) {
            continue;
        }
        if !src.exists() {
            // 原始位置没有、分区里也没有 → 文件已经被删掉或挪去别处了，记录可以清掉，
            // 免得历史无限增长
            if !dst.exists() {
                let _ = db.delete_move_history(id);
            }
            continue;
        }
        // 已经在分区里（上次是崩溃退出）→ 不用动
        if dst.exists() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            // 名单里已存在的是受管目录；不存在才新建，新建后才需要打隐藏
            let created = !parent.exists();
            let _ = fs::create_dir_all(parent);
            if created {
                hide_dir(parent);
            }
        }
        if fs::rename(src, dst).is_ok() {
            moved += 1;
        }
    }
    Ok(moved)
}

/// 把 `src` 移动到**精确路径** `dst`（不做重名去重，调用方自己保证目标不冲突）。
/// 跨盘同样会自动退化成"复制 + 删源"。
pub fn transfer_to_exact(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        return Err(format!("{} 已存在", dst.display()));
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let is_dir = src.is_dir();
    match fs::rename(src, dst) {
        Ok(_) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            if is_dir {
                copy_dir_all(src, dst)?;
                fs::remove_dir_all(src).map_err(|re| format!("原目录删除失败：{re}"))?;
            } else {
                fs::copy(src, dst).map_err(|ce| format!("复制失败：{ce}"))?;
                fs::remove_file(src).map_err(|re| format!("原文件删除失败：{re}"))?;
            }
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

fn fallback_box_id(boxes: &[(i64, String, String)]) -> Option<i64> {
    boxes
        .iter()
        .find(|(_, _, t)| t == "file")
        .map(|(id, _, _)| *id)
        .or_else(|| boxes.iter().find(|(_, _, t)| t != "folder").map(|(id, _, _)| *id))
}

pub fn organize_desktop(db: &Db) -> Result<OrganizeResult, String> {
    let include_shortcuts = db
        .get_setting("organizeShortcuts")?
        .map(|v| v == "true")
        .unwrap_or(false);
    let boxes = load_boxes_with_id(db)?;
    // 由同一份数据推导桌面 box 名，避免再扫一遍 boxes 表
    let box_names: Vec<String> = boxes.iter().map(|(_, n, _)| n.clone()).collect();
    let rules = load_enabled_rules(db)?;
    let desktop_files = scan_desktop(&box_names, include_shortcuts)?;
    let mut result = OrganizeResult::new();
    for file in &desktop_files {
        let target_box = if include_shortcuts && file.ext == "lnk" {
            boxes.iter().find(|(_, _, t)| t == "software").map(|(id, _, _)| *id)
        } else {
            match_file(file, &rules, None).or_else(|| fallback_box_id(&boxes))
        };
        let Some(box_id) = target_box else { continue };
        let Some((_, box_name, box_type)) = boxes.iter().find(|(id, _, _)| *id == box_id) else {
            continue;
        };
        // 快捷文件夹分区只做链接展示，不作为整理目标
        if box_type == "folder" {
            continue;
        }
        match ensure_box_dir(box_name).and_then(|dir| move_file(db, Path::new(&file.path), &dir)) {
            Ok(_) => {
                result.moved += 1;
                if let Some(d) = result.details.iter_mut().find(|d| d.box_name == *box_name) {
                    d.count += 1;
                } else {
                    result.details.push(OrganizeDetail { box_name: box_name.clone(), count: 1 });
                }
            }
            // 失败不再吞掉：带上文件名和原因（占用/权限/不存在），前端能弹出来
            Err(e) => {
                result.failed += 1;
                result.errors.push(format!("「{}」{}", file.name, e));
            }
        }
    }
    Ok(result)
}

// ---------- 自动整理：预览 → 确认执行 → 可撤销 ----------
//
// 以前"一键整理"是「扫一遍 → 直接搬」，用户对"哪些文件会被动"零感知：
// 刚下载到一半的文件可能被搬走、桌面上正在编辑的文件可能被吸进分区。
// 现在拆成三步：plan（只算不动）→ 前端弹预览、用户可勾选 → commit（逐项执行、
// 逐条报错）→ undo（按批次记录原路搬回）。

/// 预览里的一条：这个文件将被搬去哪个分区、依据是什么。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePlanItem {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub size: u64,
    pub target_box_id: i64,
    pub target_box_name: String,
    /// 匹配依据，可直接展示：`扩展名 png` / `关键词 报告` / `软件快捷方式` / `未匹配规则，进默认分区`
    pub matched_by: String,
}

/// 一次整理的预览结果。**只做匹配，不动任何文件。**
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizePlan {
    pub items: Vec<OrganizePlanItem>,
    /// 正在被占用 / 刚写完的文件：这一轮不动（避免搬走写了一半的内容），单独列出
    pub busy: Vec<String>,
}

/// 前端确认后回传的执行清单（用户可能取消了部分勾选）
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeCommitItem {
    pub path: String,
    pub target_box_id: i64,
}

/// 执行结果。`batch_id` 指向本次批次（organize_log），供"撤销上次整理"使用；
/// 一项都没搬成功时为 None。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizeCommitResult {
    pub moved: i64,
    pub failed: i64,
    pub errors: Vec<String>,
    pub batch_id: Option<String>,
}

pub fn plan_organize_desktop(db: &Db) -> Result<OrganizePlan, String> {
    let include_shortcuts = db
        .get_setting("organizeShortcuts")?
        .map(|v| v == "true")
        .unwrap_or(false);
    let boxes = load_boxes_with_id(db)?;
    let box_names: Vec<String> = boxes.iter().map(|(_, n, _)| n.clone()).collect();
    let rules = load_enabled_rules(db)?;
    let desktop_files = scan_desktop(&box_names, include_shortcuts)?;
    let fallback = fallback_box_id(&boxes).and_then(|id| {
        boxes
            .iter()
            .find(|(bid, _, t)| *bid == id && t != "folder")
            .map(|(id, n, _)| (*id, n.clone()))
    });
    let mut plan = OrganizePlan { items: Vec::new(), busy: Vec::new() };
    for file in &desktop_files {
        let src = Path::new(&file.path);
        // 预览阶段就把"正在写入"的文件剔出来：它们被单独展示成"跳过"，
        // 而不是混在要搬的列表里让用户误以为会被搬走
        if is_file_busy(src) {
            plan.busy.push(file.name.clone());
            continue;
        }
        let target: Option<(i64, String, String)> = if include_shortcuts && file.ext == "lnk" {
            boxes
                .iter()
                .find(|(_, _, t)| t == "software")
                .map(|(id, n, _)| (*id, n.clone(), "软件快捷方式".to_string()))
        } else {
            match match_file_detailed(file, &rules, None) {
                Some((id, note)) => {
                    // 规则指向快捷文件夹分区时不作数（它只做链接展示），退回默认分区
                    let is_folder = boxes
                        .iter()
                        .any(|(bid, _, t)| *bid == id && t == "folder");
                    if is_folder {
                        fallback.clone().map(|(id, n)| (id, n, note))
                    } else {
                        boxes
                            .iter()
                            .find(|(bid, _, _)| *bid == id)
                            .map(|(_, n, _)| (id, n.clone(), note))
                    }
                }
                None => fallback
                    .clone()
                    .map(|(id, n)| (id, n, "未匹配规则，进默认分区".to_string())),
            }
        };
        let Some((box_id, box_name, note)) = target else { continue };
        plan.items.push(OrganizePlanItem {
            path: file.path.clone(),
            name: file.name.clone(),
            ext: file.ext.clone(),
            size: file.size,
            target_box_id: box_id,
            target_box_name: box_name,
            matched_by: note,
        });
    }
    Ok(plan)
}

/// 执行整理：把前端确认过的清单逐项搬进目标分区。
///
/// 与预览之间可能隔了几分钟，源文件的状态可能已变 —— 所以每一项都**重新检查**：
/// 目标分区还在不在、源文件还在不在、是否被占用。失败的给**逐条可读原因**，
/// 绝不静默跳过。搬成功的写进一个批次（organize_log），撤销时按它原路搬回。
pub fn commit_organize_desktop(
    db: &Db,
    items: &[OrganizeCommitItem],
) -> Result<OrganizeCommitResult, String> {
    let boxes = load_boxes_with_id(db)?;
    let mut r = OrganizeCommitResult { moved: 0, failed: 0, errors: Vec::new(), batch_id: None };
    let mut log: Vec<(String, String)> = Vec::new();
    for item in items {
        let src = Path::new(&item.path);
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| item.path.clone());
        let Some((_, box_name, box_type)) =
            boxes.iter().find(|(id, _, _)| *id == item.target_box_id)
        else {
            r.failed += 1;
            r.errors.push(format!("「{name}」的目标分区已被删除"));
            continue;
        };
        if box_type == "folder" {
            r.failed += 1;
            r.errors.push(format!("「{name}」的目标是快捷文件夹，不能作为整理落点"));
            continue;
        }
        if !src.exists() {
            r.failed += 1;
            r.errors.push(format!("「{name}」已不存在（可能被移动或删除）"));
            continue;
        }
        if is_file_busy(src) {
            r.failed += 1;
            r.errors.push(format!("「{name}」正在被其他程序使用，稍后再试"));
            continue;
        }
        match ensure_box_dir(box_name).and_then(|dir| move_file(db, src, &dir)) {
            Ok(to) => {
                r.moved += 1;
                log.push((item.path.clone(), to.to_string_lossy().to_string()));
            }
            Err(e) => {
                r.failed += 1;
                r.errors.push(format!("「{name}」：{e}"));
            }
        }
    }
    if !log.is_empty() {
        let batch_id = format!("org-{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"));
        db.organize_log_insert_batch(&batch_id, &log)?;
        r.batch_id = Some(batch_id);
    }
    Ok(r)
}

/// 撤销最近一次整理：把这批文件**原路搬回**。
///
/// - 原位置没人占 → 回到原路径；
/// - 原位置被占了 → 改名放在原目录（绝不覆盖用户这几分钟新放的文件）；
/// - 源文件已被用户挪走 → 跳过（不用管）。
/// 搬回的同时清掉对应的移动历史，免得"退出归还"再按它搬一次。
///
/// 返回 (搬回数, 批次总数)。
pub fn undo_last_organize(db: &Db) -> Result<(u32, u32), String> {
    let Some((batch_id, _, _)) = db.latest_organize_batch()? else {
        return Err("没有可撤销的整理记录".to_string());
    };
    let items = db.organize_batch_items(&batch_id)?;
    let total = items.len() as u32;
    let mut back = 0u32;
    for (from, to) in &items {
        let src = Path::new(to);
        if !src.exists() {
            continue;
        }
        let fname = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let dst_dir = Path::new(from)
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| "无法获取桌面路径".to_string())?;
        let target = unique_target_kind(&dst_dir, &fname, src.is_dir());
        if transfer_to_exact(src, &target).is_ok() {
            back += 1;
            // 清掉这条搬运记录：文件已经回原位，历史里留着它只会误导后续的归还链路
            let _ = db.delete_move_by_to(to);
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(p) = src.parent() {
                refresh_shell_views(p);
            }
            if let Some(p) = target.parent() {
                refresh_shell_views(p);
            }
        }
    }
    db.organize_log_delete_batch(&batch_id)?;
    Ok((back, total))
}

/// 最近一次整理批次的信息：(batch_id, 文件数, 时间)。给"撤销上次整理"按钮显示用。
pub fn last_organize_info(db: &Db) -> Result<Option<(String, i64, String)>, String> {
    db.latest_organize_batch()
}

pub fn load_boxes_with_id(db: &Db) -> Result<Vec<(i64, String, String)>, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, name, type FROM boxes ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row.map_err(|e| e.to_string())?);
    }
    Ok(list)
}

pub fn organize_box(db: &Db, box_id: i64) -> Result<OrganizeResult, String> {
    let boxes = load_boxes_with_id(db)?;
    let Some((_, box_name, box_type)) = boxes.iter().find(|(id, _, _)| *id == box_id) else {
        return Err("分区不存在".to_string());
    };
    if box_type == "folder" {
        return Ok(OrganizeResult::new());
    }
    let box_dir = ensure_box_dir(box_name)?;
    let rules = load_enabled_rules(db)?;
    let mut result = OrganizeResult::new();
    let entries = fs::read_dir(&box_dir).map_err(|e| format!("读取分区失败: {e}"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if is_hidden_or_system(&meta) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        let file = DesktopFile {
            name: name.clone(),
            path: path.to_string_lossy().to_string(),
            is_dir: false,
            ext,
            size: meta.len(),
            modified_at: modified,
            created_at: String::new(),
        };
        let Some(target_id) = match_file(&file, &rules, None) else { continue };
        if target_id == box_id {
            continue;
        }
        let Some((_, target_name, target_type)) = boxes.iter().find(|(id, _, _)| *id == target_id) else {
            continue;
        };
        if target_type == "folder" {
            continue;
        }
        match ensure_box_dir(target_name).and_then(|dir| move_file(db, &path, &dir)) {
            Ok(_) => {
                result.moved += 1;
                if let Some(d) = result.details.iter_mut().find(|d| d.box_name == *target_name) {
                    d.count += 1;
                } else {
                    result.details.push(OrganizeDetail { box_name: target_name.clone(), count: 1 });
                }
            }
            Err(e) => {
                result.failed += 1;
                result.errors.push(format!("「{}」{}", name, e));
            }
        }
    }
    Ok(result)
}

/// 文件是否正被其他程序写入：独占打开失败，或刚刚才被改动。
/// 用来避开「正在复制/下载中的文件」，免得吸到写了一半的内容。
fn is_file_busy(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::{Duration, SystemTime};

    // 1) 以「不允许任何共享」方式打开：正在写入的文件会返回共享冲突
    if fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .is_err()
    {
        return true;
    }
    // 2) 刚出现的文件先放过一轮，给复制/保存留出缓冲
    if let Ok(meta) = fs::metadata(path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = SystemTime::now().duration_since(modified) {
                if age < Duration::from_millis(1200) {
                    return true;
                }
            }
        }
    }
    false
}

/// 按分区的吸附配置，把桌面上扩展名命中的文件收进该分区。
/// 只处理桌面根目录下的文件，不会碰其它分区文件夹。
pub fn absorb_into_box(db: &Db, box_id: i64) -> Result<OrganizeResult, String> {
    let mut result = OrganizeResult::new();
    let boxes = load_boxes_with_id(db)?;
    let Some((_, box_name, box_type)) = boxes.iter().find(|(id, _, _)| *id == box_id) else {
        return Err("分区不存在".to_string());
    };
    if box_type == "folder" {
        return Ok(result);
    }
    let exts = db.get_absorb_cfg(box_id)?.exts;
    if exts.is_empty() {
        return Ok(result);
    }
    let dir = ensure_box_dir(box_name)?;
    let names = db.box_names()?;
    // include_shortcuts = true：勾了 lnk 时也要能把桌面快捷方式吸进来
    for file in scan_desktop(&names, true)? {
        if !exts.iter().any(|e| e == &file.ext) {
            continue;
        }
        let src = Path::new(&file.path);
        if is_file_busy(src) {
            continue;
        }
        match move_file(db, src, &dir) {
            Ok(_) => {
                result.moved += 1;
                if let Some(d) = result.details.iter_mut().find(|d| d.box_name == *box_name) {
                    d.count += 1;
                } else {
                    result
                        .details
                        .push(OrganizeDetail { box_name: box_name.clone(), count: 1 });
                }
            }
            Err(e) => {
                result.failed += 1;
                result.errors.push(format!("「{}」{}", file.name, e));
            }
        }
    }
    Ok(result)
}

pub fn get_box_files(box_name: &str, desc: bool) -> Result<Vec<DesktopFile>, String> {
    let dir = desktop_dir()?.join(box_name);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&dir).map_err(|e| format!("读取分区失败: {e}"))?;
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "desktop.ini" {
            continue;
        }
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        let created = meta
            .created()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        files.push(DesktopFile {
            name,
            path: path.to_string_lossy().to_string(),
            is_dir: meta.is_dir(),
            ext,
            size: meta.len(),
            modified_at: modified,
            created_at: created,
        });
    }
    sort_files(&mut files, "name", desc);
    Ok(files)
}

/// 调用 reg.exe 等控制台程序时带上，避免 GUI 进程弹出一个控制台黑窗
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn set_auto_start(enable: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe_path = format!("\"{}\"", exe.display());
    let mut cmd = std::process::Command::new("reg");
    if enable {
        cmd.args(["add", key, "/v", "Qidomin", "/t", "REG_SZ", "/d", &exe_path, "/f"]);
    } else {
        cmd.args(["delete", key, "/v", "Qidomin", "/f"]);
    }
    // 不加这个标志，每次调用都会闪一个控制台窗口
    cmd.creation_flags(CREATE_NO_WINDOW);
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!("注册表操作失败: {}", String::from_utf8_lossy(&o.stderr))),
        Err(e) => Err(e.to_string()),
    }
}
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Qidomin";

/// 读取 Run 项里 Qidomin 的值（reg query 输出里 REG_SZ 之后的那段）
fn read_run_value() -> Option<String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", RUN_VALUE])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    for line in text.lines() {
        let line = line.trim();
        if let Some(i) = line.find("REG_SZ") {
            return Some(line[i + "REG_SZ".len()..].trim().to_string());
        }
    }
    None
}

/// 开机自启自愈。
///
/// 两个历史坑：① 老版本把路径写成**不带引号**的形式，路径里一旦有空格，
/// Windows 解析就会失败 → "开了开机启动却没反应"；② 程序换过目录后，Run 项
/// 仍指向已经不存在的旧 exe（本机实测指向被删掉的 `桌面\qidomin.exe`）。
/// 启动时对一次：只要设置里开着自启、而 Run 值不等于当前 exe 的带引号路径，就重写。
/// 返回是否真的做了修复。
pub fn repair_auto_start(db: &Db) -> bool {
    let enabled = db
        .get_setting("app")
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|v| v.get("autoStart").and_then(|b| b.as_bool()))
        .unwrap_or(false);
    if !enabled {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let want = format!("\"{}\"", exe.display());
    if read_run_value().as_deref() == Some(want.as_str()) {
        return false;
    }
    set_auto_start(true).is_ok()
}

/// 列出文件夹内容（快捷文件夹分区用），支持排序（文件夹优先）
/// sort_mode: name / time / size / type
pub fn list_folder_sorted(path: &Path, sort_mode: &str, desc: bool) -> Result<Vec<DesktopFile>, String> {
    if !path.exists() {
        return Err(format!("文件夹不存在: {}", path.display()));
    }
    let entries = fs::read_dir(path).map_err(|e| format!("读取文件夹失败: {e}"))?;
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if is_hidden_or_system(&meta) {
            continue;
        }
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "desktop.ini" {
            continue;
        }
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta
            .modified()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        let created = meta
            .created()
            .ok()
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_default();
        files.push(DesktopFile {
            name,
            path: p.to_string_lossy().to_string(),
            is_dir: meta.is_dir(),
            ext,
            size: meta.len(),
            modified_at: modified,
            created_at: created,
        });
    }
    sort_files(&mut files, sort_mode, desc);
    // 完整返回文件夹里所有文件，不加数量上限
    Ok(files)
}

/// 排序：仅“名称”模式把文件夹置顶；按修改时间/大小/类型排时纯按字段，
/// 这样“刚加进来的文件”能排到最前（用户要求）。desc = 降序（最新的/最大的在前）。
fn sort_files(files: &mut [DesktopFile], sort_mode: &str, desc: bool) {
    files.sort_by(|a, b| {
        // 只有按名称排才把文件夹钉在最前，其余模式不做文件夹置顶
        if sort_mode == "name" {
            let dir_cmp = b.is_dir.cmp(&a.is_dir);
            if dir_cmp != std::cmp::Ordering::Equal {
                return dir_cmp;
            }
        }
        let cmp = match sort_mode {
            // 按"创建时间"排（= 文件进入该文件夹的时刻）：Windows 复制/移动会保留旧
            // 的"修改时间"，只有创建时间能反映"刚被加进来"，所以用 created_at；
            // 同秒创建的再用修改时间、名称兜底，保证顺序稳定不跳。
            "time" => a
                .created_at
                .cmp(&b.created_at)
                .then_with(|| a.modified_at.cmp(&b.modified_at))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            "size" => a
                .size
                .cmp(&b.size)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            "type" => a
                .ext
                .cmp(&b.ext)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if desc {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 取出路径的盘符，用来判断两个目录是否在不同卷
    fn volume_of(p: &Path) -> Option<char> {
        p.to_string_lossy()
            .chars()
            .next()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
    }

    /// 跨盘（不同卷）搬运：源必须消失、目标必须到位，且不能报"删源失败"。
    ///
    /// 锁的是这个回归：以前跨盘走"自己 copy → 再删源"，删源那步会报
    /// `系统找不到指定的文件 (os error 2)`，于是给用户弹一条
    /// "已复制到目标分区，但原文件删除失败，请手动删除"的**假错误**。
    /// 现在走 `MoveFileExW(MOVEFILE_COPY_ALLOWED)`，由系统一步搬完。
    ///
    /// 机器上只有一个卷时自动跳过（不算失败），目录用完即删。
    #[test]
    fn cross_volume_move_moves_file() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let temp = match std::env::var("TEMP") {
            Ok(t) => PathBuf::from(t),
            Err(_) => {
                eprintln!("skip: 取不到 TEMP");
                return;
            }
        };
        match (volume_of(&here), volume_of(&temp)) {
            (Some(a), Some(b)) if a != b => {}
            _ => {
                eprintln!("skip: 只有一个卷（{here:?} 与 {temp:?}）");
                return;
            }
        }

        let src_dir = here.join("target").join("qd_xvol_src");
        let dst_dir = temp.join("qd_xvol_dst");
        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_dir_all(&dst_dir);
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();

        let name = "tb787584090-0911-0.stl";
        let file = src_dir.join(name);
        fs::write(&file, b"solid test\n").unwrap();

        let moved = transfer(&file, &dst_dir, false).expect("跨盘移动应当成功");
        assert_eq!(moved, dst_dir.join(name));
        assert!(moved.exists(), "目标文件应当存在");
        assert!(!file.exists(), "源文件应当已经消失（跨盘移动 = 复制 + 删源）");
        assert_eq!(fs::read(&moved).unwrap(), b"solid test\n");

        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_dir_all(&dst_dir);
    }

    /// 落点被占（并发搬运 / 重名撞车）时：换个名字搬过去，
    /// **绝不覆盖**已经存在的那个文件。机器上只有一个卷时自动跳过。
    #[test]
    fn cross_volume_move_does_not_overwrite_occupied_target() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let temp = match std::env::var("TEMP") {
            Ok(t) => PathBuf::from(t),
            Err(_) => {
                eprintln!("skip: 取不到 TEMP");
                return;
            }
        };
        match (volume_of(&here), volume_of(&temp)) {
            (Some(a), Some(b)) if a != b => {}
            _ => {
                eprintln!("skip: 只有一个卷");
                return;
            }
        }

        let src_dir = here.join("target").join("qd_xvol_src2");
        let dst_dir = temp.join("qd_xvol_dst2");
        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_dir_all(&dst_dir);
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();

        let name = "occupied.stl";
        let file = src_dir.join(name);
        fs::write(&file, b"new content\n").unwrap();
        // 目标位置上已经有一个同名文件（内容不同）—— 它必须原封不动
        let occupied = dst_dir.join(name);
        fs::write(&occupied, b"original\n").unwrap();

        let landed = move_across_volumes(&file, &occupied, false).expect("应换个名字搬过去");
        assert_ne!(landed, occupied, "不许覆盖已存在的落点");
        assert!(landed.exists(), "换名后的落点应当存在");
        assert_eq!(fs::read(&landed).unwrap(), b"new content\n");
        assert_eq!(
            fs::read(&occupied).unwrap(),
            b"original\n",
            "原来的那个文件不能被覆盖"
        );
        assert!(!file.exists(), "源文件应当已经搬走");

        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_dir_all(&dst_dir);
    }

    /// 同一次拖拽被两路同时处理时，重复的一路必须**跳过**，不能算失败 ——
    /// 以前它会报"复制 xxx 失败：系统找不到指定的文件"，而文件明明已经搬好了。
    #[test]
    fn transfer_into_box_skips_duplicate_and_missing_source() {
        let db_file = std::env::temp_dir().join("qidomin_xfer_test.db");
        let _ = fs::remove_file(&db_file);
        let db = crate::db::Db::open(&db_file).expect("open db");

        let dst_dir = std::env::temp_dir().join("qd_xfer_box_dir");
        let src_dir = std::env::temp_dir().join("qd_xfer_src_dir");
        let _ = fs::remove_dir_all(&dst_dir);
        let _ = fs::remove_dir_all(&src_dir);
        fs::create_dir_all(&dst_dir).unwrap();
        fs::create_dir_all(&src_dir).unwrap();

        // 用"关联文件夹"分区：落点直接指向临时目录，不会去碰真实的桌面
        let box_id: i64 = {
            let c = db.conn.lock().unwrap();
            c.execute(
                "INSERT INTO boxes (name, color, type, link_path, created_at) \
                 VALUES ('qd_xfer', '#ffffff', 'folder', ?1, '2026-01-01 00:00:00')",
                rusqlite::params![dst_dir.to_string_lossy()],
            )
            .unwrap();
            c.last_insert_rowid()
        };

        // 1) 源已经不在（被另一路搬走了）→ 跳过，不算失败
        let gone = src_dir.join("qd_gone.stl");
        let r = transfer_into_box(&db, box_id, &[gone.to_string_lossy().into_owned()], false)
            .expect("整体应当成功");
        assert_eq!(r.skipped, 1, "源不存在应当跳过");
        assert_eq!(r.failed, 0, "源不存在不该算失败");

        // 2) 同一路径连着搬两次 → 第二次被去重跳过（3 秒窗口内）
        let file = src_dir.join("qd_dup.stl");
        fs::write(&file, b"solid\n").unwrap();
        let p = file.to_string_lossy().into_owned();
        let r1 = transfer_into_box(&db, box_id, &[p.clone()], false).unwrap();
        assert_eq!((r1.moved, r1.failed), (1, 0), "第一次应当正常搬过去");
        let r2 = transfer_into_box(&db, box_id, &[p.clone()], false).unwrap();
        assert_eq!(r2.skipped, 1, "重复的一路应当被跳过");
        assert_eq!(r2.failed, 0, "重复的一路不该报失败");
        assert_eq!(fs::read(dst_dir.join("qd_dup.stl")).unwrap(), b"solid\n");

        let _ = fs::remove_dir_all(&dst_dir);
        let _ = fs::remove_dir_all(&src_dir);
        let _ = fs::remove_file(&db_file);
    }
}