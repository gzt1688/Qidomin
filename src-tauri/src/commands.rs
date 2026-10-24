use crate::boxwin;
use crate::db::{
    AbsorbCfg, BoxWindowCfg, Db, HuangdiWindowCfg, HuangliWindowCfg, Note, NoteTab, NoteWindowCfg,
    PoetryWindowCfg, ShanghanWindowCfg, StockItem, StockWindowCfg,
};
use crate::stock::Quote;
use crate::engine::{self, DesktopFile, OrganizeResult};
use crate::icons;
use chrono::Local;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxInfo {
    pub id: i64,
    pub name: String,
    pub color: String,
    pub file_count: u64,
    pub box_type: String,
    pub link_path: Option<String>,
    pub sort_mode: String,
    pub sort_desc: bool,
    /// 饼型分区嵌套在哪个分区里（0 = 独立分区）
    #[serde(default)]
    pub nested_in: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleInfo {
    pub id: i64,
    pub name: String,
    pub box_id: i64,
    pub box_name: String,
    pub match_type: String,
    pub pattern: String,
    pub priority: i64,
    pub enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub organize_shortcuts: bool,
    pub auto_start: bool,
    pub real_time_organize: bool,
    pub snap_enabled: bool,
    pub default_icon_size: i32,
    #[serde(default = "default_box_bg_color")]
    pub box_bg_color: String,
    #[serde(default)]
    pub box_bg_opacity: i32,
    /// 分区标题字体颜色（CSS 颜色值），默认绿色
    #[serde(default = "default_box_title_color")]
    pub box_title_color: String,
    /// 显示桌面（Win+D）时是否保持组件/分区可见（挂为桌面子窗口免疫最小化）
    #[serde(default = "default_keep_on_show_desktop")]
    pub keep_on_show_desktop: bool,
    /// 分区里的文件名是否隐藏后缀名（默认显示）
    #[serde(default)]
    pub hide_file_extensions: bool,
    /// 搜索索引是否覆盖全盘（默认只索引常用目录）
    #[serde(default)]
    pub full_disk_index: bool,
    /// 需要额外索引的盘符列表（如 ["C","D"]），为空则只索引常用目录
    #[serde(default)]
    pub index_drives: Vec<String>,
    /// 搜索结果排序：mtime（修改时间最新在前，默认）/ name（文件名）/ size（大小）
    #[serde(default = "default_search_sort")]
    pub search_sort: String,
    /// 应用内界面缩放（1.0 = 100%），嫌系统字号小可单独放大
    #[serde(default = "default_ui_zoom")]
    pub ui_zoom: f64,
    /// 鼠标快捷键总开关（默认关闭：不抢占任何鼠标键）
    #[serde(default)]
    pub mouse_hotkeys_enabled: bool,
    /// 鼠标快捷键映射表（旧设置无该字段时预置中键 → 快速搜索）
    #[serde(default = "default_mouse_hotkeys")]
    pub mouse_hotkeys: Vec<crate::mousehotkey::MouseBinding>,
    /// 饼型菜单圆钮直径（逻辑像素，40–96，默认 54）
    #[serde(default = "default_radial_btn_size")]
    pub radial_btn_size: i32,
    /// 饼型菜单弹出区直径/宽度基准（逻辑像素，280–720，默认 440）
    #[serde(default = "default_radial_expand_size")]
    pub radial_expand_size: i32,
    /// 饼型弹出文件气泡/卡片的边长（逻辑像素，44–110，默认 66）
    #[serde(default = "default_radial_item_size")]
    pub radial_item_size: i32,
    /// 饼型弹出文件之间的最小间距（逻辑像素，10–120，默认 28）。
    /// 环半径按「文件数 × (文件尺寸+间距)」的周长需求自动算：
    /// 文件少时贴近中心、多时自动外扩，任何数量都不会重叠。
    #[serde(default = "default_radial_ring_gap")]
    pub radial_ring_gap: i32,
    /// 饼型菜单标题：空 = 用分区名；非空时圆钮和中心钮都显示它
    #[serde(default)]
    pub radial_title: String,
    /// 饼型菜单图标：图片 data URL 或绝对路径；空 = 不显示图标（显示标题）
    #[serde(default)]
    pub radial_icon: String,
}

fn default_radial_btn_size() -> i32 {
    54
}

fn default_radial_expand_size() -> i32 {
    440
}

fn default_radial_item_size() -> i32 {
    66
}

fn default_radial_ring_gap() -> i32 {
    28
}

fn default_mouse_hotkeys() -> Vec<crate::mousehotkey::MouseBinding> {
    // 预置两条：
    // ① 中键（后再按空格）→ 快速搜索。中键按下完整放行给软件（3D 旋转视图等不冲突），
    //    短时间内按空格才触发；
    // ② 双击中键 → 显示 / 隐藏全部分区。双击连按很快，单击绑定会自动让位（见 mousehotkey.rs）
    vec![
        crate::mousehotkey::MouseBinding {
            id: 1,
            button: "middle".into(),
            ctrl: false,
            shift: false,
            alt: false,
            space: false,
            mode: "spaceAfter".into(),
            action: "toggleSearch".into(),
            enabled: true,
        },
        crate::mousehotkey::MouseBinding {
            id: 2,
            button: "middle2".into(),
            ctrl: false,
            shift: false,
            alt: false,
            space: false,
            mode: String::new(),
            action: "toggleBoxes".into(),
            enabled: true,
        },
    ]
}

/// 老用户迁移：设置里只有老版那条默认「中键+空格」时，补上新的默认
/// 「双击中键 → 显示/隐藏分区」。用户只要改过映射表（条目数/内容对不上老默认）
/// 就一律不动，尊重用户自己的配置。
fn migrate_mouse_hotkeys(mut list: Vec<crate::mousehotkey::MouseBinding>) -> Vec<crate::mousehotkey::MouseBinding> {
    let is_old_default = list.len() == 1
        && list[0].id == 1
        && list[0].button == "middle"
        && list[0].mode == "spaceAfter"
        && list[0].action == "toggleSearch";
    if is_old_default {
        list.extend(default_mouse_hotkeys().into_iter().skip(1));
    }
    list
}

fn default_ui_zoom() -> f64 {
    1.0
}

fn default_box_bg_color() -> String {
    "#ffffff".into()
}

fn default_box_title_color() -> String {
    "#1a9c5d".into()
}

fn default_keep_on_show_desktop() -> bool {
    true
}

fn default_search_sort() -> String {
    "mtime".into()
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            organize_shortcuts: false,
            auto_start: false,
            real_time_organize: false,
            snap_enabled: true,
            default_icon_size: 48,
            box_bg_color: default_box_bg_color(),
            box_bg_opacity: 0,
            box_title_color: default_box_title_color(),
            keep_on_show_desktop: true,
            hide_file_extensions: false,
            full_disk_index: false,
            index_drives: Vec::new(),
            search_sort: default_search_sort(),
            ui_zoom: default_ui_zoom(),
            mouse_hotkeys_enabled: false,
            mouse_hotkeys: default_mouse_hotkeys(),
            radial_btn_size: default_radial_btn_size(),
            radial_expand_size: default_radial_expand_size(),
            radial_item_size: default_radial_item_size(),
            radial_ring_gap: default_radial_ring_gap(),
            radial_title: String::new(),
            radial_icon: String::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IconItem {
    pub name: String,
    pub path: String,
    pub ext: String,
    pub is_dir: bool,
    pub icon: String,
    /// 嵌套饼型分区的条目：真实分区 id（path 是 `box:{id}` 虚拟值，不是文件）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nested_box_id: Option<i64>,
    /// 嵌套饼型分区自己的颜色（画圆钮底色用；普通文件条目无此字段）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

fn count_dir_entries(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().to_lowercase() != "desktop.ini")
        .count() as u64
}

pub(crate) fn get_box_name_color(db: &Db, id: i64) -> Result<(String, String), String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT name, color FROM boxes WHERE id = ?1",
        params![id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .map_err(|_| "分区不存在".to_string())
}

/// 校验并插入一个新分区行（不含建目录 / 挂监视 / 开窗口），返回新 id。
/// 被 `create_box` 命令与 `lib.rs::create_box_and_open`（启动参数建分区）共用，
/// 避免两份重复的「同名校验 + 插入」逻辑。
pub(crate) fn insert_box_row(
    db: &Db,
    name: &str,
    color: &str,
    box_type: &str,
    link_path: Option<&str>,
) -> Result<i64, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("分区名称不能为空".to_string());
    }
    let link_path = link_path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    // 桌面同名检查只对**要建桌面文件夹的**分区类型有意义。
    // 关联文件夹（folder）分区不建任何目录，名字只是面板标题，桌面有没有同名文件夹
    // 都与它无关 —— 以前无条件拦，等于白白禁掉一批本来完全可用的名字。
    if box_type != "folder" && engine::desktop_dir()?.join(&name).exists() {
        return Err(format!("桌面已存在同名文件夹「{name}」"));
    }
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM boxes WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists > 0 {
        return Err("已存在同名分区".to_string());
    }
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    conn.execute(
        "INSERT INTO boxes (name, color, type, link_path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![name, color, box_type, link_path, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// 定下新分区最终叫什么名字。
///
/// 调用方给的名字能用就用；**已被占用时自动换成同族的可用名字，而不是报错**。
///
/// 以前前端按"同类型分区个数 + 1"自己拼名字：7 个关联文件夹分区算出下一个是
/// "文件夹 8"，可库里恰好已经有个"文件夹 8"（分区会解散、会改名，个数和最大序号
/// 早就对不上了），于是 `insert_box_row` 报"已存在同名分区"，而前端只 `console.error`，
/// 用户看到的就是"点新建分区/关联文件夹，什么反应都没有"。
///
/// 同族延续：从"文件夹 8"里取出前缀"文件夹"，接着往下取号 → "文件夹 9"。
pub(crate) fn resolve_box_name(db: &Db, requested: &str, box_type: &str) -> Result<String, String> {
    let need_dir = box_type != "folder";
    let requested = requested.trim();
    if !requested.is_empty() && engine::box_name_available(db, requested, need_dir)? {
        return Ok(requested.to_string());
    }
    // 关联文件夹分区的名字来自真实文件夹名，**重名时只加序号**（"资料" → "资料 2"），
    // 不要退化成"文件夹 N"。下面的前缀逻辑会把名字里的数字全切掉
    // （"照片2024" → 前缀"照片" → "照片 1"），对文件夹名来说是灾难。
    if box_type == "folder" && !requested.is_empty() {
        let name = engine::pick_free_box_name_same(db, requested)?;
        crate::log_msg(&format!("关联文件夹名「{requested}」已被占用，改用「{name}」"));
        return Ok(name);
    }
    let fallback = match box_type {
        "folder" => "文件夹",
        "software" => "软件",
        "radial" => "饼型菜单",
        _ => "文件",
    };
    let prefix: String = requested
        .chars()
        .take_while(|c| !c.is_ascii_digit())
        .collect::<String>()
        .trim()
        .to_string();
    let prefix = if prefix.is_empty() {
        fallback.to_string()
    } else {
        prefix
    };
    let name = engine::pick_free_box_name(db, &prefix, need_dir)?;
    if !requested.is_empty() {
        crate::log_msg(&format!("分区名「{requested}」已被占用，自动改用「{name}」"));
    }
    Ok(name)
}

/// 名字是不是系统自动取的那种（`文件夹 3`）。
///
/// 用来判断"这个名字用户自己动过没有"：只有没动过的（自动取的），
/// 关联文件夹改链时才让面板名跟着新文件夹走。
fn is_auto_folder_name(name: &str) -> bool {
    match name.strip_prefix("文件夹 ") {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

#[tauri::command]
pub fn get_desktop_files(db: State<Db>) -> Result<Vec<DesktopFile>, String> {
    let include_shortcuts = db
        .get_setting("organizeShortcuts")?
        .map(|v| v == "true")
        .unwrap_or(false);
    let names = db.box_names()?;
    engine::scan_desktop(&names, include_shortcuts)
}

#[tauri::command]
pub fn get_boxes(db: State<Db>) -> Result<Vec<BoxInfo>, String> {
    let list = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color, type, link_path, sort_mode, sort_desc, nested_in FROM boxes ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)? != 0,
                    r.get::<_, i64>(7)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        list
    };
    let desktop = engine::desktop_dir()?;
    Ok(list
        .into_iter()
        .map(|(id, name, color, box_type, link_path, sort_mode, sort_desc, nested_in)| BoxInfo {
            // 快捷访问分区统计的是它链接的那个真实目录，别去数桌面上的同名文件夹
            file_count: if box_type == "folder" {
                link_path
                    .as_ref()
                    .map(|p| count_dir_entries(Path::new(p)))
                    .unwrap_or(0)
            } else {
                count_dir_entries(&desktop.join(&name))
            },
            id,
            name,
            color,
            box_type,
            link_path,
            sort_mode,
            sort_desc,
            nested_in,
        })
        .collect())
}

#[tauri::command]
pub fn get_box_by_id(db: State<Db>, id: i64) -> Result<BoxInfo, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    let row = conn
        .query_row(
            "SELECT name, color, type, link_path, sort_mode, sort_desc, nested_in FROM boxes WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)? != 0,
                    r.get::<_, i64>(6)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?;
    drop(conn);
    let (name, color, box_type, link_path, sort_mode, sort_desc, nested_in) = row;
    let file_count = if box_type == "folder" {
        link_path
            .as_ref()
            .map(|p| count_dir_entries(Path::new(p)))
            .unwrap_or(0)
    } else {
        let desktop = engine::desktop_dir()?;
        count_dir_entries(&desktop.join(&name))
    };
    Ok(BoxInfo {
        id,
        name,
        color,
        file_count,
        box_type,
        link_path,
        sort_mode,
        sort_desc,
        nested_in,
    })
}

/// 分区窗口的图标列表。
///
/// 这里**不提取任何图片**：只做"列目录 + 排序 + 拼图标 URL"。真正的缩略图提取由
/// `qidominicon://` 协议按需触发（前端 `loading="lazy"`，只有看得见的那几张会来取）。
/// 之前是把 base64 直接塞进返回值，一个几百项的文件夹每次刷新要搬好几 MB 字符串，
/// 还要 `JSON.parse` + 逐张解码，界面必卡。
///
/// 列目录是阻塞 IO（大目录还要 stat 每一条），所以仍然丢进 spawn_blocking。
#[tauri::command]
pub async fn get_box_icons(db: State<'_, Db>, box_id: i64) -> Result<Vec<IconItem>, String> {
    // 一次查询拿全部分区元信息（类型/链接/排序/名称），避免两次加锁
    let (box_type, link_path, sort_mode, desc, box_name) = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT type, link_path, sort_mode, sort_desc, name FROM boxes WHERE id = ?1",
            params![box_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)? != 0,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(|_| "分区不存在".to_string())?
    };
    // 饼型圆钮图**每分区独立**（BoxWindowCfg.radial_icon）；分区没设过
    // （空串）回退全局默认（app 设置 JSON 的 radialIcon 字段）。
    // **先读 cfg 和设置再锁 conn**：两者内部也要锁 db.conn，持锁期间再调
    // 它们会同线程死锁（分区全空的事故就是这么来的）。
    let radial_icon = match db.get_box_window_cfg(box_id) {
        Ok(Some(cfg)) if !cfg.radial_icon.is_empty() => cfg.radial_icon,
        _ => db
            .get_setting("app")
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| {
                v.get("radialIcon")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_default(),
    };
    // 嵌进本分区的饼型菜单（虚拟条目，宿主列表里要显示）
    let nested_list: Vec<(i64, String, String)> = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name, color FROM boxes WHERE nested_in = ?1 AND type = 'radial' ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![box_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(|r| r.ok()).collect()
    };
    let _ = box_name;
    // 关联文件夹分区：列它链接的那个文件夹；普通分区：列分区自己的文件夹
    let is_folder = box_type == "folder" && link_path.as_ref().map(|l| !l.is_empty()).unwrap_or(false);
    let (link_path, sort_mode) = if is_folder {
        (link_path.clone().unwrap_or_default(), sort_mode)
    } else {
        (String::new(), String::new())
    };
    let box_name = if is_folder { String::new() } else { box_name };

    tauri::async_runtime::spawn_blocking(move || {
        let files = if is_folder {
            engine::list_folder_sorted(Path::new(&link_path), &sort_mode, desc)?
        } else {
            engine::get_box_files(&box_name, desc)?
        };
        // 统一按 96px 请求图标：分区图标最大可调到 96
        let mut out: Vec<IconItem> = files
            .into_iter()
            .map(|f| IconItem {
                icon: icons::icon_url(&f.path, 96),
                name: f.name,
                path: f.path,
                ext: f.ext,
                is_dir: f.is_dir,
                nested_box_id: None,
                color: None,
            })
            .collect();
        // 拼上嵌套的饼型菜单条目：path 用 `box:{id}` 虚拟值（绝不与真实文件同名，
        // 退出归还/搬移等一切按路径操作的逻辑都必须跳过它）。图标复用全局饼型圆钮图，
        // 颜色用该饼型分区自己的（前端画圆底）。
        for (nid, nname, ncolor) in nested_list {
            out.push(IconItem {
                icon: radial_icon.clone(),
                name: nname,
                path: format!("box:{nid}"),
                ext: "radial".into(),
                is_dir: false,
                nested_box_id: Some(nid),
                color: Some(ncolor),
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 新建分区。**返回最终用的分区名** —— 前端拿它弹提示，也是"后端才是命名的唯一权威"
/// 的体现：名字被占用时后端会自动换号，前端不需要（也不应该）自己拼名字。
///
/// `name` 为空 → 后端按类型自动取名（"文件 N" / "文件夹 N" / "软件 N"）。
#[tauri::command]
// async 命令：在 tokio 线程执行，避免 Windows 上 WebView2 在 IPC 线程创建窗口的死锁
// （见 tauri-webview_window::new 文档与 wry#583）
pub async fn create_box(
    app: AppHandle,
    db: State<'_, Db>,
    name: Option<String>,
    color: String,
    box_type: Option<String>,
    link_path: Option<String>,
) -> Result<String, String> {
    let box_type = box_type.unwrap_or_else(|| "file".to_string());
    let link_path = link_path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    let mut requested = name.unwrap_or_default().trim().to_string();
    // 关联文件夹分区没指定名字 → **默认就用被关联文件夹的名字**
    // （"I:\资料\项目资料" → "项目资料"）。面板名和它指向的文件夹同名，用户一眼就知道
    // 这块面板是哪个文件夹的快捷访问；重名时由 resolve_box_name 加序号，不会退化成"文件夹 N"。
    if requested.is_empty() && box_type == "folder" {
        if let Some(folder) = link_path.as_deref().and_then(engine::folder_display_name) {
            requested = folder;
        }
    }
    let name = resolve_box_name(&db, &requested, &box_type)?;
    let box_id = insert_box_row(&db, &name, &color, &box_type, link_path.as_deref())?;
    // 饼型菜单分区：先落一份"圆钮 + 默认样式(pie)"的窗口配置，再建窗。
    // 建窗逻辑按 radial_style 非空 + collapsed=true 走圆钮尺寸分支。
    if box_type == "radial" {
        db.update_box_window_cfg(box_id, |c| {
            c.radial_style = "pie".into();
            c.collapsed = true;
            // 圆钮比默认分区小得多，往屏幕中间挪一点，别贴在 (60,60) 角上
            c.x = 120;
            c.y = 120;
        })?;
    }
    if box_type != "folder" {
        match engine::ensure_box_dir(&name) {
            Ok(dir) => {
                // 普通分区目录也要挂实时监视：资源管理器 / 系统右键菜单在这里增删改名，
                // 面板得跟着变（以前只有关联文件夹被监视，普通分区只能靠"下一次操作"刷）
                app.state::<crate::folder_watch::FolderWatcher>()
                    .add(box_id, &dir.to_string_lossy());
            }
            Err(e) => {
                // 目录没建起来就别把这条分区留在库里 —— 留一条没有文件夹的"僵尸分区"，
                // 既显示不出来，又会让"新建分区"的序号继续往上跑。
                let _ = db
                    .conn
                    .lock()
                    .map(|c| c.execute("DELETE FROM boxes WHERE id = ?1", params![box_id]));
                return Err(e);
            }
        }
    }
    // 关联文件夹：挂上实时监视
    if box_type == "folder" {
        if let Some(lp) = &link_path {
            app.state::<crate::folder_watch::FolderWatcher>().add(box_id, lp);
        }
    }
    // 创建并显示分区窗口（async 命令在 tokio 线程，窗口经内部调度在主线程创建，无死锁）
    crate::log_msg(&format!("create_box {name} (id={box_id}, type={box_type}) insert ok, build window"));
    if let Err(e) = boxwin::open_box_window(&app, &db, box_id, &name, &color) {
        crate::log_msg(&format!("create_box {name} 建窗失败：{e}"));
        return Err(format!("分区「{name}」已创建，但窗口打不开：{e}"));
    }
    crate::log_msg(&format!("create_box {name} window built"));
    // 显示失败不算"新建失败"（分区和窗口都已经在那儿了），记日志即可 ——
    // 报错会让用户以为没建成，然后又点一次，白白多出一个分区。
    if let Err(e) = boxwin::set_box_visible(&app, box_id, true) {
        crate::log_msg(&format!("create_box {name} 显示窗口失败：{e}"));
    }
    Ok(name)
}

/// 解散分区（「文件退回桌面」那一档）：按移动历史送回整理前的原始位置，
/// 手动拖进来的退回桌面。**不碰 boxes 表**，由调用方决定何时清理 DB 行。
///
/// 三条硬规矩：
/// 1. **只对"我们自己的"分区文件夹生效**（带 HIDDEN|SYSTEM）。目录不存在、或存在
///    但明显是用户自己在桌面上建的文件夹 → 立刻返回，一个文件都不动、属性也不改。
/// 2. 归属判定按**父目录精确匹配**，不用字符串前缀 —— 以前用前缀，
///    分区「工作」会顺带吃掉「工作日历」里的文件。
/// 3. 退回之后目录里还有东西（文件被占用等）→ 报错说明剩下什么，让用户能处理；
///    目录删不掉时**取消隐藏**，绝不在桌面上留一个用户看不见的残留目录。
fn restore_box_files(db: &Db, name: &str) -> Result<(), String> {
    let desktop = engine::desktop_dir()?;
    let box_dir = desktop.join(name);
    if !engine::is_managed_box_dir(&box_dir) {
        return Ok(());
    }

    // 1) 按移动历史退回：文件回到整理前的原始位置
    let history = db.get_move_history()?;
    let mut failed: Vec<String> = Vec::new();
    for (hid, _fname, from, to) in &history {
        let src = Path::new(to);
        if src.parent() != Some(box_dir.as_path()) {
            continue;
        }
        if !src.exists() {
            let _ = db.delete_move_history(*hid);
            continue;
        }
        // 归还目标统一由 engine::return_target_for 决定：
        // 原始位置可用 → 回那儿；不可用（历史脏数据 / 指向别的分区隐藏目录）→ 回桌面根目录。
        // 以前是直接拿 `from` 当目标，脏数据会让文件被搬进另一个看不见的隐藏文件夹，
        // 甚至原地改名（`from == to` 时），用户看到的就是"解散了但文件没回来"。
        let target = engine::return_target_for(db, &desktop, src, from);
        match engine::transfer_to_exact(src, &target) {
            Ok(_) => {
                let _ = db.delete_move_history(*hid);
            }
            Err(e) => failed.push(format!("{} ({e})", src.display())),
        }
    }

    // 2) 未被历史记录的（用户手动拖入分区的）→ 退回桌面。
    //    先收集再搬：一边读目录一边往外移，枚举有漏项的风险。
    let rest: Vec<(String, std::path::PathBuf, bool)> = fs::read_dir(&box_dir)
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
    for (fname, p, is_dir) in rest {
        let target = engine::unique_target_kind(&desktop, &fname, is_dir);
        match engine::transfer_to_exact(&p, &target) {
            Ok(_) => {}
            Err(e) => failed.push(format!("{fname} ({e})")),
        }
    }

    // 3) 清算结果：还剩东西就说清楚，别假装成功
    let leftovers: Vec<String> = fs::read_dir(&box_dir)
        .map(|it| {
            it.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| !n.eq_ignore_ascii_case("desktop.ini"))
                .collect()
        })
        .unwrap_or_default();
    if !leftovers.is_empty() {
        return Err(format!(
            "分区「{name}」还有 {} 项没能退出来（可能正被别的程序占用）：{}。\
             分区未解散，处理掉之后可以再试一次。",
            leftovers.len(),
            leftovers.join("、")
        ));
    }
    if fs::remove_dir(&box_dir).is_err() {
        // 删不掉（比如里面留了个 desktop.ini）→ 至少取消隐藏，
        // 绝不在桌面上留一个用户看不见、也进不去的目录。
        // 取消隐藏失败也要说出来 —— 用户看到的现象正是"文件夹不见了"。
        if !engine::unhide_dir(&box_dir) {
            return Err(format!(
                "分区「{name}」的文件夹没能删掉，也没能恢复显示（{}）。\
                 里面的文件都还在，可以手动改一下它的隐藏属性。",
                box_dir.display()
            ));
        }
    }
    if !failed.is_empty() {
        return Err(format!(
            "{} 个文件退回原位置失败：{}",
            failed.len(),
            failed.join("; ")
        ));
    }
    Ok(())
}

/// 解散分区（「只解散分组」那一档）：**文件原地不动**，只把分区文件夹交还给用户。
///
/// 具体做两件事，都不涉及删除或移动任何文件：
/// 1. 取消 HIDDEN|SYSTEM —— 桌面上能直接看到、双击能进去；
/// 2. 清掉这批文件在 move_history 里的记录 —— 否则程序下次启动 / 退出会按记录
///    把它们又搬回隐藏目录里去，"交还"就成了摆设。
///
/// 同样只对"我们自己的"分区文件夹生效。
///
/// 取消隐藏必须**确认成功**再往上报成"解散完成"：写属性失败（或资源管理器没刷新）
/// 时用户看到的还是"分区没了、文件夹也不见了"，那是最坏的结果 —— 宁可报错让他重试。
fn detach_box_dir(db: &Db, name: &str) -> Result<(), String> {
    let dir = engine::desktop_dir()?.join(name);
    if !engine::is_managed_box_dir(&dir) {
        return Ok(());
    }
    if !engine::unhide_dir(&dir) {
        return Err(format!(
            "分区「{name}」的文件夹没能恢复显示（{}）。文件都还在里面，\
             可以手动取消它的隐藏属性，或稍后再试一次。",
            dir.display()
        ));
    }
    let n = db.delete_move_history_under(&dir.to_string_lossy())?;
    crate::log_msg(&format!("分区「{name}」已解散分组：文件原地保留，清理 {n} 条搬运记录"));
    Ok(())
}

/// 解散分区。
///
/// 三种情形严格区分（这是"解散 = 只解散分组"的落地点）：
/// - **快捷访问 / 关联文件夹分区**（`folder`）：只撤掉这块"快捷方式"（DB 行 + 窗口）。
///   它链接的那个真实文件夹、以及里面的东西**一律不碰** —— 不搬、不删、不改属性。
/// - **普通分区 + `restore_files = true`**：分区里的文件按历史退回原来的位置。
/// - **普通分区 + `restore_files = false`**：文件原地不动，把分区文件夹取消隐藏交还用户。
#[tauri::command]
pub async fn delete_box(
    app: AppHandle,
    db: State<'_, Db>,
    id: i64,
    restore_files: bool,
) -> Result<(), String> {
    let (name, _) = get_box_name_color(&db, id)?;
    let box_type = db
        .box_type_info(id)
        .map_err(|e| e.to_string())?
        .map(|(t, _, _, _)| t)
        .unwrap_or_else(|| "file".to_string());
    if box_type == "folder" {
        // 关联文件夹分区：链接出去的是用户自己的目录，解散不允许对它有任何副作用。
        // 以前这里会走 restore_box_files —— 万一这个分区名和桌面上某个真实文件夹
        // 重名，那一整个文件夹的内容都会被搬到桌面根目录再被删掉。
        crate::log_msg(&format!("解散快捷访问分区「{name}」：关联文件夹保持原样"));
    } else if restore_files {
        restore_box_files(&db, &name)?;
    } else {
        detach_box_dir(&db, &name)?;
    }
    // 解散宿主前先把里面嵌套的饼型分区全部放回桌面（它们恢复独立，
    // 否则 nested_in 指向一个已删除的分区，圆钮永远不会再出现）
    unnest_all_in(&app, &db, id);
    {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM rules WHERE box_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM boxes WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    // 取消该分区的文件夹实时监视
    app.state::<crate::folder_watch::FolderWatcher>().remove(id);
    boxwin::destroy_box_window(&app, id);
    Ok(())
}

#[tauri::command]
pub fn rename_box(app: AppHandle, db: State<Db>, id: i64, name: String) -> Result<(), String> {
    let new_name = name.trim().to_string();
    if new_name.is_empty() {
        return Err("分区名称不能为空".to_string());
    }
    let (old_name, _) = get_box_name_color(&db, id)?;
    if old_name == new_name {
        return Ok(());
    }
    {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM boxes WHERE name = ?1 AND id != ?2",
                params![new_name, id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if exists > 0 {
            return Err("已存在同名分区".to_string());
        }
    }
    let desktop = engine::desktop_dir()?;
    let old_dir = desktop.join(&old_name);
    let new_dir = desktop.join(&new_name);
    // 目标名撞上桌面上已有的文件夹 → 直接拒绝。放过去的话，这个分区名就和用户的
    // 真实文件夹同名了，之后任何"按分区名拼路径"的操作（其中不少是搬空 / 删目录）
    // 都可能落到用户文件夹上。
    if new_dir.exists() {
        return Err(format!("桌面已存在同名文件夹「{new_name}」，请换个名字"));
    }
    // 只重命名我们自己的分区文件夹；不是我们的（用户自己在桌面建的）一律不动，
    // 只改数据库里的名字 —— 这个分区下次用到时会重新建自己的文件夹。
    if engine::is_managed_box_dir(&old_dir) {
        fs::rename(&old_dir, &new_dir).map_err(|e| format!("重命名文件夹失败: {e}"))?;
        // 目录换路径了，监视器得跟着换 —— 否则这个分区从此收不到刷新事件，
        // 又变成"改了不更新"的老毛病
        app.state::<crate::folder_watch::FolderWatcher>()
            .set_link(id, &new_dir.to_string_lossy());
    } else if old_dir.exists() {
        crate::log_msg(&format!(
            "分区「{old_name}」的文件夹不是本程序创建的，重命名时保持原样"
        ));
    }
    db.conn
        .lock()
        .map_err(|e| e.to_string())?
        .execute("UPDATE boxes SET name = ?1 WHERE id = ?2", params![new_name, id])
        .map_err(|e| e.to_string())?;
    boxwin::rename_box_window(&app, id, &new_name);
    Ok(())
}

#[tauri::command]
pub fn merge_boxes(
    app: AppHandle,
    db: State<Db>,
    src_id: i64,
    dst_id: i64,
) -> Result<(), String> {
    if src_id == dst_id {
        return Err("不能合并到自身".to_string());
    }
    let (src_name, _) = get_box_name_color(&db, src_id)?;
    // 目标分区必须存在（顺便做一次存在性校验）
    get_box_name_color(&db, dst_id)?;
    let desktop = engine::desktop_dir()?;
    let src_dir = desktop.join(&src_name);
    // 目标落点必须走 box_dir_for：合并目标是"关联文件夹"分区时，文件该落进它链接的
    // 那个真实目录。以前固定用 ensure_box_dir(分区名)，会在桌面偷偷建一个隐藏文件夹
    // 把所有文件塞进去，而那个分区展示的却是链接目录 —— 用户眼里文件就是凭空没了。
    let dst_dir = engine::box_dir_for(&db, dst_id)?;
    // 源目录不是我们建的就一个文件都不动
    if engine::is_managed_box_dir(&src_dir) {
        // 先收集再搬：一边读目录一边往外移，枚举有漏项的风险
        let items: Vec<(String, std::path::PathBuf)> = fs::read_dir(&src_dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter_map(|entry| {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.eq_ignore_ascii_case("desktop.ini") {
                    return None;
                }
                Some((fname, entry.path()))
            })
            .collect();
        for (fname, p) in items {
            // 统一走 engine::move_file：跨盘、文件夹、移动记录续接都由它处理
            engine::move_file(&db, &p, &dst_dir)
                .map_err(|e| format!("移动 {fname} 失败: {e}"))?;
        }
        // 空了才删；删不掉就取消隐藏，别留一个看不见的残留目录
        if fs::remove_dir(&src_dir).is_err() {
            engine::unhide_dir(&src_dir);
        }
    }
    // 合并会删掉源分区：先把里面嵌套的饼型分区放回桌面，避免 nested_in 悬空
    unnest_all_in(&app, &db, src_id);
    {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM rules WHERE box_id = ?1", params![src_id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM boxes WHERE id = ?1", params![src_id])
            .map_err(|e| e.to_string())?;
    }
    app.state::<crate::folder_watch::FolderWatcher>().remove(src_id);
    boxwin::destroy_box_window(&app, src_id);
    // 通知目标分区刷新
    if let Some(label) = app.state::<boxwin::BoxWindows>().label_for(dst_id) {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.emit("box-files-changed", dst_id);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn organize_desktop(db: State<Db>) -> Result<OrganizeResult, String> {
    engine::organize_desktop(&db)
}

/// 自动整理**预览**：只算"谁会被搬到哪"，不动任何文件。
/// 前端弹预览对话框，用户确认（可取消勾选）后再调 organize_desktop_commit。
#[tauri::command]
pub fn organize_desktop_plan(db: State<Db>) -> Result<engine::OrganizePlan, String> {
    engine::plan_organize_desktop(&db)
}

/// 执行用户确认过的整理清单，返回批次 id 供撤销。
#[tauri::command]
pub fn organize_desktop_commit(
    db: State<Db>,
    items: Vec<engine::OrganizeCommitItem>,
) -> Result<engine::OrganizeCommitResult, String> {
    engine::commit_organize_desktop(&db, &items)
}

/// 撤销最近一次整理：按批次记录原路搬回，返回 (搬回数, 批次总数)。
#[tauri::command]
pub fn undo_last_organize(db: State<Db>) -> Result<(u32, u32), String> {
    engine::undo_last_organize(&db)
}

/// 最近一次整理批次的信息，没有则为 null。
#[tauri::command]
pub fn get_last_organize_batch(db: State<Db>) -> Result<Option<(String, i64, String)>, String> {
    engine::last_organize_info(&db)
}

#[tauri::command]
pub fn organize_box(db: State<Db>, box_id: i64) -> Result<OrganizeResult, String> {
    engine::organize_box(&db, box_id)
}

// ---------- 分区：吸附同类文件 ----------

#[tauri::command]
pub fn get_absorb_cfg(db: State<Db>, box_id: i64) -> Result<AbsorbCfg, String> {
    db.get_absorb_cfg(box_id)
}

/// 保存吸附配置并立刻整理一次，返回本次收进来的文件数
#[tauri::command]
pub fn set_absorb_cfg(
    app: AppHandle,
    db: State<Db>,
    box_id: i64,
    exts: Vec<String>,
    auto: bool,
) -> Result<OrganizeResult, String> {
    db.set_absorb_cfg(box_id, &exts, auto)?;
    // 唤醒自动吸附线程：开启了自动吸附时要立刻开始工作，
    // 关闭了（最后一个）也要让它重新判断并挂起，别再空转
    crate::absorb::wake();
    let result = engine::absorb_into_box(&db, box_id)?;
    notify_box_changed(&app, box_id);
    Ok(result)
}

/// 按当前配置再吸附一次（手动触发）
#[tauri::command]
pub fn absorb_box_now(app: AppHandle, db: State<Db>, box_id: i64) -> Result<OrganizeResult, String> {
    let result = engine::absorb_into_box(&db, box_id)?;
    notify_box_changed(&app, box_id);
    Ok(result)
}

/// 通知某个分区面板刷新文件列表
fn notify_box_changed(app: &AppHandle, box_id: i64) {
    if let Some(label) = app.state::<boxwin::BoxWindows>().label_for(box_id) {
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.emit("box-files-changed", box_id);
        }
    }
}

#[tauri::command]
pub fn set_box_link(
    app: AppHandle,
    db: State<Db>,
    id: i64,
    path: String,
) -> Result<(), String> {
    let path = path.trim().to_string();
    // 空路径 = 解除链接：卸掉监视、清空配置
    if path.is_empty() {
        db.set_box_link(id, "")?;
        app.state::<crate::folder_watch::FolderWatcher>()
            .set_link(id, "");
        notify_box_changed(&app, id);
        return Ok(());
    }
    // 规范化路径（消解 .. 与符号链接），并校验是一个真实存在的目录，
    // 同时拒绝系统关键目录，防止前端被绕过时挂载到系统目录做实时监视。
    let canon = Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("链接路径无效：{e}"))?;
    if !canon.is_dir() {
        return Err("链接必须是一个存在的文件夹".to_string());
    }
    if let Some(sys) = std::env::var_os("SystemRoot") {
        if canon.starts_with(Path::new(&sys)) {
            return Err("不能链接系统目录".to_string());
        }
    }
    let canon = canon.to_string_lossy().to_string();
    // 名字跟着文件夹走：关联文件夹分区的面板名**默认就是被关联文件夹的名字**。
    // 只在"名字显然还是系统自动取的（文件夹 N），或还跟着上一个文件夹"时才自动改，
    // 用户手动改过的名字一律不动。
    if let Some((_, old_link, _, _)) = db.box_type_info(id).map_err(|e| e.to_string())? {
        let (cur, _) = get_box_name_color(&db, id)?;
        let old_base = old_link.as_deref().and_then(engine::folder_display_name);
        let follows_folder = old_base.as_deref() == Some(cur.as_str()) || is_auto_folder_name(&cur);
        if follows_folder {
            if let Some(base) = engine::folder_display_name(&canon) {
                if base != cur {
                    let new_name = if engine::box_name_available(&db, &base, false)? {
                        base
                    } else {
                        engine::pick_free_box_name_same(&db, &base)?
                    };
                    db.conn
                        .lock()
                        .map_err(|e| e.to_string())?
                        .execute("UPDATE boxes SET name = ?1 WHERE id = ?2", params![new_name, id])
                        .map_err(|e| e.to_string())?;
                    boxwin::rename_box_window(&app, id, &new_name);
                    crate::log_msg(&format!(
                        "关联文件夹分区改名：{cur} → {new_name}（跟随文件夹名）"
                    ));
                }
            }
        }
    }
    db.set_box_link(id, &canon)?;
    // 改链：重新挂监视器
    app.state::<crate::folder_watch::FolderWatcher>()
        .set_link(id, &canon);
    // 通知面板立即刷新（链接可能指向不同的目录）
    notify_box_changed(&app, id);
    Ok(())
}

#[tauri::command]
pub fn set_box_sort(db: State<Db>, id: i64, sort_mode: String, desc: bool) -> Result<(), String> {
    db.set_box_sort(id, sort_mode.trim(), desc)?;
    Ok(())
}

// ---------- 布局备份 / 恢复 ----------

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutWindow {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    icon_size: i32,
    locked: bool,
    visible: bool,
    bg_color: String,
    bg_opacity: i32,
    /// 旧备份文件里没有这个字段，缺省白色
    #[serde(default = "crate::db::default_label_color")]
    label_color: String,
    /// 旧备份文件里没有这个字段，缺省图标模式
    #[serde(default = "crate::db::default_view_mode")]
    view_mode: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutBox {
    name: String,
    color: String,
    box_type: String,
    link_path: Option<String>,
    sort_mode: String,
    /// 旧备份文件里没有这个字段，缺省按升序
    #[serde(default)]
    sort_desc: bool,
    window: Option<LayoutWindow>,
}

/// 便签页（标签栏上的一页）
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutNoteTab {
    id: i64,
    name: String,
    created_at: String,
}

/// 一条便签的内容与提醒设置
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutNote {
    tab_id: i64,
    content: String,
    remind_at: Option<String>,
    remind_alarm: bool,
    fired: bool,
    created_at: String,
    /// 最后修改时间；旧备份没有这个字段，缺省空串（恢复时退回用 created_at）
    #[serde(default)]
    modified_at: String,
}

/// 分区「吸附同类文件」配置，按分区名对应（恢复时分区 id 会变）
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutAbsorb {
    box_name: String,
    exts: Vec<String>,
    auto: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LayoutFile {
    version: i32,
    app_settings: Option<serde_json::Value>,
    /// 便签窗口位置/外观。旧备份没有，缺省不恢复
    #[serde(default)]
    note_window: Option<NoteWindowCfg>,
    #[serde(default)]
    stock_window: Option<StockWindowCfg>,
    #[serde(default)]
    huangli_window: Option<HuangliWindowCfg>,
    #[serde(default)]
    rules: Vec<RuleInfo>,
    #[serde(default)]
    boxes: Vec<LayoutBox>,
    #[serde(default)]
    note_tabs: Vec<LayoutNoteTab>,
    #[serde(default)]
    notes: Vec<LayoutNote>,
    #[serde(default)]
    stocks: Vec<StockItem>,
    #[serde(default)]
    absorbs: Vec<LayoutAbsorb>,
}

/// 把当前桌面布局（分区、窗口位置/外观、整理规则、全局设置）导出为 JSON 文件
#[tauri::command]
pub fn export_layout(db: State<Db>, path: String) -> Result<(), String> {
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("保存路径不能为空".to_string());
    }
    let app_settings = db
        .get_setting("app")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let rules = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT r.id, r.name, r.box_id, IFNULL(b.name, ''), r.match_type, r.pattern, r.priority, r.enabled
                 FROM rules r LEFT JOIN boxes b ON b.id = r.box_id
                 ORDER BY r.priority ASC, r.id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(RuleInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    box_id: r.get(2)?,
                    box_name: r.get(3)?,
                    match_type: r.get(4)?,
                    pattern: r.get(5)?,
                    priority: r.get(6)?,
                    enabled: r.get::<_, i64>(7)? != 0,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        list
    };
    let boxes = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, color, type, link_path, sort_mode, sort_desc FROM boxes ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)? != 0,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        list
    };
    let mut out_boxes = Vec::new();
    let mut box_ids: Vec<(i64, String)> = Vec::new();
    for (id, name, color, box_type, link_path, sort_mode, sort_desc) in boxes {
        let window = db
            .get_box_window_cfg(id)
            .ok()
            .flatten()
            .map(|c| LayoutWindow {
                x: c.x,
                y: c.y,
                w: c.w,
                h: c.h,
                icon_size: c.icon_size,
                locked: c.locked,
                visible: c.visible,
                bg_color: c.bg_color,
                bg_opacity: c.bg_opacity,
                label_color: c.label_color,
                view_mode: c.view_mode,
            });
        box_ids.push((id, name.clone()));
        out_boxes.push(LayoutBox {
            name,
            color,
            box_type,
            link_path,
            sort_mode,
            sort_desc,
            window,
        });
    }
    // 便签：页 + 内容（用户最常改的"数据"，之前没进备份）
    let note_tabs = db
        .conn
        .lock()
        .map_err(|e| e.to_string())?
        .prepare("SELECT id, name, created_at FROM note_tabs ORDER BY id")
        .map_err(|e| e.to_string())?
        .query_map([], |r| {
            Ok(LayoutNoteTab {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    let notes = db
        .conn
        .lock()
        .map_err(|e| e.to_string())?
        .prepare(
            "SELECT tab_id, content, remind_at, remind_alarm, fired, created_at, modified_at \
             FROM notes ORDER BY id",
        )
        .map_err(|e| e.to_string())?
        .query_map([], |r| {
            Ok(LayoutNote {
                tab_id: r.get(0)?,
                content: r.get(1)?,
                remind_at: r.get::<_, Option<String>>(2)?,
                remind_alarm: r.get::<_, i64>(3)? != 0,
                fired: r.get::<_, i64>(4)? != 0,
                created_at: r.get(5)?,
                modified_at: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect::<Vec<_>>();
    // 股票自选
    let stocks = db.list_stocks().unwrap_or_default();
    // 各组件窗口配置
    let note_window = db.get_note_window_cfg().ok().flatten();
    let stock_window = db.get_stock_window_cfg().ok().flatten();
    let huangli_window = db.get_huangli_window_cfg().ok().flatten();
    // 分区吸附配置：按分区名对应（恢复时分区 id 会变）
    let mut absorbs = Vec::new();
    for (id, name) in &box_ids {
        if let Some(cfg) = db.get_absorb_cfg(*id).ok() {
            absorbs.push(LayoutAbsorb {
                box_name: name.clone(),
                exts: cfg.exts,
                auto: cfg.auto,
            });
        }
    }
    let file = LayoutFile {
        version: 1,
        app_settings,
        note_window,
        stock_window,
        huangli_window,
        rules,
        boxes: out_boxes,
        note_tabs,
        notes,
        stocks,
        absorbs,
    };
    let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    fs::write(&path, json).map_err(|e| format!("保存失败: {e}"))?;
    Ok(())
}

/// 从备份 JSON 恢复桌面布局。
///
/// 关键设计（修复之前"半残状态丢数据"的风险）：
/// 1) 先**整体校验**备份结构，任何不合法都直接报错返回，绝不碰当前数据；
/// 2) 再清掉当前分区文件（退回桌面）——这步失败也只动文件，DB 仍是原样，可重试；
/// 3) 所有 DB 写入（分区 / 便签 / 股票 / 规则 / 设置 / 窗口配置）放进**同一笔事务**，
///    中途出错自动回滚，不会出现"DB 清了却没重建完"的半残状态；
/// 4) 事务提交后才打开窗口、挂监视器等纯副作用。
#[tauri::command]
// async 命令：避免 Windows 上 WebView2 在 IPC 线程创建窗口的死锁（同 create_box）
pub async fn import_layout(app: AppHandle, db: State<'_, Db>, path: String) -> Result<(), String> {
    let raw = fs::read_to_string(path.trim()).map_err(|e| format!("读取备份文件失败: {e}"))?;
    let file: LayoutFile =
        serde_json::from_str(&raw).map_err(|e| format!("备份文件格式错误: {e}"))?;

    // 1) 整体校验，任何不合法都在此返回，绝不开始破坏当前布局
    validate_layout(&file)?;

    // 2) 记录当前分区（id + 名字 + 类型，仅用于提交后决定是否把旧文件夹退回桌面）。
    //    这里**不删除任何数据**——删除 / 重建全部放进下面的事务，
    //    事务失败会自动回滚，绝不会停留在「当前布局已删、新布局没建好」的半残状态。
    //    旧分区 id 还用来关掉它们残留的窗口（见第 4 步），否则会变成幽灵窗口。
    let old_boxes: Vec<(i64, String, String)> = {
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
        rows.filter_map(|r| r.ok()).collect()
    };
    let new_names: std::collections::HashSet<String> =
        file.boxes.iter().map(|b| b.name.trim().to_string()).collect();

    // 3) 所有 DB 写入放进同一笔事务：中途失败自动回滚
    let mut name_to_id: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    {
        let mut conn = db.conn.lock().map_err(|e| e.to_string())?;
        import_layout_db(&mut *conn, &file, &mut name_to_id)?;
    }

    // 分区窗口配置已统一走 Db 的锁内读-改-写，不存在内存缓存需要清理

    // 4) 事务已提交：**先关掉旧分区的窗口**，再打开新分区的窗口。
    //    导入把 boxes 表整体重建、id 全部换新，旧窗口如果不关就直接变成"幽灵面板"：
    //    库里已经没有它的记录了，谁也管不到它，它就永远浮在桌面上（拖不动、改不了、
    //    右键菜单也打不开），而且每次导入都会再积一批。
    for (old_id, old_name, _) in &old_boxes {
        boxwin::destroy_box_window(&app, *old_id);
        crate::log_msg(&format!(
            "导入布局：关闭旧分区窗口 {old_name}（id={old_id}）"
        ));
    }

    // 5) 打开新分区的窗口 + 挂监视（纯副作用，失败不影响已落盘的 DB）
    for b in &file.boxes {
        let Some(&new_id) = name_to_id.get(&b.name) else { continue };
        if b.box_type == "folder" {
            if let Some(lp) = &b.link_path {
                if !lp.trim().is_empty() {
                    app.state::<crate::folder_watch::FolderWatcher>()
                        .add(new_id, lp);
                }
            }
        } else {
            // 普通分区：建/复用目录之后同样挂上实时监视
            if let Ok(dir) = engine::ensure_box_dir(&b.name) {
                app.state::<crate::folder_watch::FolderWatcher>()
                    .add(new_id, &dir.to_string_lossy());
            }
        }
        let _ = boxwin::open_box_window(&app, &db, new_id, &b.name, &b.color);
    }

    // 6) 事务已提交、DB 已落盘：此时才处理文件。把「不在新布局里的旧分区文件夹」
    //    退回桌面（同名分区保留，文件夹复用）。这步在 DB 写入之后，任何失败都只影响文件，
    //    绝不丢 DB 数据——即使上一步事务失败，当前布局也完好无损。
    //    快捷访问分区（folder）只撤掉那条记录，链接出去的真实目录一律不动。
    for (_, old, old_type) in &old_boxes {
        if old_type == "folder" || new_names.contains(old) {
            continue;
        }
        // restore_box_files 内部只对"我们自己建的分区文件夹"生效，
        // 同名但属于用户的文件夹会被它直接跳过。
        let _ = restore_box_files(&db, old);
    }
    Ok(())
}

/// 校验备份文件结构：任何不合法都返回错误，绝不开始破坏当前布局。
fn validate_layout(file: &LayoutFile) -> Result<(), String> {
    if file.version != 1 {
        return Err(format!("不支持的备份版本：{}", file.version));
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for b in &file.boxes {
        let name = b.name.trim();
        if name.is_empty() {
            return Err("备份中存在空名称的分区".to_string());
        }
        if b.color.trim().is_empty() {
            return Err(format!("分区「{name}」缺少颜色配置"));
        }
        match b.box_type.as_str() {
            "file" | "folder" | "software" => {}
            other => return Err(format!("分区「{name}」类型非法：{other}")),
        }
        if b.box_type == "folder" && b.link_path.as_ref().map(|p| p.trim().is_empty()).unwrap_or(true)
        {
            return Err(format!("关联文件夹分区「{name}」缺少链接路径"));
        }
        if !seen.insert(name.to_string()) {
            return Err(format!("备份中存在重名分区：「{name}」"));
        }
    }
    for t in &file.note_tabs {
        if t.name.trim().is_empty() {
            return Err("备份中存在空名称的便签页".to_string());
        }
    }
    Ok(())
}

/// 在同一把连接锁内、同一笔事务里写入导入所需的全部行。
/// 调用方需已持有 `conn`（即已 lock），这里不再自行加锁，避免死锁。
fn import_layout_db(
    conn: &mut rusqlite::Connection,
    file: &LayoutFile,
    name_to_id: &mut std::collections::HashMap<String, i64>,
) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // 先清旧数据（整体替换语义）
    tx.execute("DELETE FROM rules", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM notes", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM note_tabs", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM stocks", []).map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM boxes", []).map_err(|e| e.to_string())?;

    // 分区（记录 name -> 新 id，供规则 / 吸附 / 窗口配置映射）
    for b in &file.boxes {
        tx.execute(
            "INSERT INTO boxes (name, color, type, link_path, created_at, sort_mode, sort_desc) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                b.name.trim(),
                b.color,
                b.box_type,
                b.link_path,
                now,
                b.sort_mode,
                if b.sort_desc { 1 } else { 0 }
            ],
        )
        .map_err(|e| e.to_string())?;
        name_to_id.insert(b.name.clone(), tx.last_insert_rowid());
    }

    // 分区窗口配置：整体替换 boxWindows 设置
    let mut cfgs: Vec<BoxWindowCfg> = Vec::new();
    for b in &file.boxes {
        if let Some(w) = &b.window {
            let id = *name_to_id.get(&b.name).unwrap_or(&0);
            if id == 0 {
                continue;
            }
            cfgs.push(BoxWindowCfg {
                id,
                x: w.x,
                y: w.y,
                w: w.w,
                h: w.h,
                icon_size: w.icon_size,
                locked: w.locked,
                visible: w.visible,
                bg_color: w.bg_color.clone(),
                bg_opacity: w.bg_opacity,
                label_color: w.label_color.clone(),
                view_mode: w.view_mode.clone(),
                // 手工排列顺序不进备份：恢复后按当前排序方式重新排列
                item_order: Vec::new(),
                gap_x: 6,
                gap_y: 6,
                // 备份/恢复始终落在"展开态"：expanded_geom 是临时状态，不跨备份
                collapsed: false,
                expanded_geom: None,
                capsule_hover: true,
                radial_style: String::new(),
                radial_show_title: true,
                radial_show_names: true,
                // 分区图标不进备份（旧备份结构无此字段）：恢复后回退全局默认
                radial_icon: String::new(),
                // 屏幕绑定不进备份：恢复时显示器拓扑可能完全不同，
                // 带着旧绑定反而会把分区摆到错误的位置
                monitor_key: String::new(),
                rel: None,
                rest_wh: None,
            });
        }
    }
    let json = serde_json::to_string(&cfgs).map_err(|e| e.to_string())?;
    Db::set_setting_with(&tx, "boxWindows", &json)?;

    // 吸附配置：落在 boxes 行的列上
    for a in &file.absorbs {
        if let Some(&id) = name_to_id.get(&a.box_name) {
            tx.execute(
                "UPDATE boxes SET absorb_exts = ?1, absorb_auto = ?2 WHERE id = ?3",
                rusqlite::params![a.exts.join(","), if a.auto { 1 } else { 0 }, id],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    // 便签页
    let mut tab_map: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    for t in &file.note_tabs {
        tx.execute(
            "INSERT INTO note_tabs (name, created_at) VALUES (?1, ?2)",
            rusqlite::params![t.name, t.created_at],
        )
        .map_err(|e| e.to_string())?;
        tab_map.insert(t.id, tx.last_insert_rowid());
    }
    // 兜底页：没有任何页时建一个，避免便签无家可归
    let fallback_tab = if file.note_tabs.is_empty() {
        tx.execute(
            "INSERT INTO note_tabs (name, created_at) VALUES ('便签', ?1)",
            rusqlite::params![now],
        )
        .map_err(|e| e.to_string())?;
        tx.last_insert_rowid()
    } else {
        0
    };

    // 便签
    for n in &file.notes {
        let new_tab = tab_map.get(&n.tab_id).copied().unwrap_or(fallback_tab);
        // 旧备份没有 modifiedAt：用创建时间顶上，别让便签丢了时间显示
        let modified = if n.modified_at.trim().is_empty() {
            n.created_at.clone()
        } else {
            n.modified_at.clone()
        };
        tx.execute(
            "INSERT INTO notes (content, remind_at, remind_alarm, fired, created_at, tab_id, modified_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                n.content,
                n.remind_at,
                if n.remind_alarm { 1 } else { 0 },
                if n.fired { 1 } else { 0 },
                n.created_at,
                new_tab,
                modified
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    // 股票自选
    for s in &file.stocks {
        tx.execute(
            "INSERT INTO stocks (code, secid, name, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![s.code, s.secid, s.name, s.sort, now],
        )
        .map_err(|e| e.to_string())?;
    }

    // 整理规则：按分区名映射到新的 id（修复旧实现直接复用备份里过期 id 的 bug）
    for r in &file.rules {
        let Some(&bid) = name_to_id.get(&r.box_name) else {
            continue;
        };
        tx.execute(
            "INSERT INTO rules (name, box_id, match_type, pattern, priority, enabled) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                r.name,
                bid,
                r.match_type,
                r.pattern,
                r.priority,
                if r.enabled { 1 } else { 0 }
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    // 全局设置
    if let Some(s) = &file.app_settings {
        Db::set_setting_with(&tx, "app", &s.to_string())?;
    }
    if let Some(w) = &file.note_window {
        let j = serde_json::to_string(w).map_err(|e| e.to_string())?;
        Db::set_setting_with(&tx, "noteWindow", &j)?;
    }
    if let Some(w) = &file.stock_window {
        let j = serde_json::to_string(w).map_err(|e| e.to_string())?;
        Db::set_setting_with(&tx, "stockWindow", &j)?;
    }
    if let Some(w) = &file.huangli_window {
        let j = serde_json::to_string(w).map_err(|e| e.to_string())?;
        Db::set_setting_with(&tx, "huangliWindow", &j)?;
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_rules(db: State<Db>) -> Result<Vec<RuleInfo>, String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT r.id, r.name, r.box_id, IFNULL(b.name, ''), r.match_type, r.pattern, r.priority, r.enabled
             FROM rules r LEFT JOIN boxes b ON b.id = r.box_id
             ORDER BY r.priority ASC, r.id ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(RuleInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                box_id: r.get(2)?,
                box_name: r.get(3)?,
                match_type: r.get(4)?,
                pattern: r.get(5)?,
                priority: r.get(6)?,
                enabled: r.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut rules = Vec::new();
    for row in rows {
        rules.push(row.map_err(|e| e.to_string())?);
    }
    Ok(rules)
}

#[tauri::command]
pub fn save_rule(db: State<Db>, rule: RuleInfo) -> Result<(), String> {
    let conn = db.conn.lock().map_err(|e| e.to_string())?;
    let enabled = if rule.enabled { 1 } else { 0 };
    if rule.id == 0 {
        conn.execute(
            "INSERT INTO rules (name, box_id, match_type, pattern, priority, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![rule.name, rule.box_id, rule.match_type, rule.pattern, rule.priority, enabled],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "UPDATE rules SET name = ?1, box_id = ?2, match_type = ?3, pattern = ?4, priority = ?5, enabled = ?6
             WHERE id = ?7",
            params![rule.name, rule.box_id, rule.match_type, rule.pattern, rule.priority, enabled, rule.id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn delete_rule(db: State<Db>, id: i64) -> Result<(), String> {
    db.conn
        .lock()
        .map_err(|e| e.to_string())?
        .execute("DELETE FROM rules WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_settings(db: State<Db>) -> Result<AppSettings, String> {
    match db.get_setting("app")? {
        Some(json) => {
            let mut s: AppSettings = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            s.mouse_hotkeys = migrate_mouse_hotkeys(s.mouse_hotkeys);
            Ok(s)
        }
        None => Ok(AppSettings::default()),
    }
}

#[tauri::command]
pub fn save_settings(app: AppHandle, db: State<Db>, settings: AppSettings) -> Result<(), String> {
    let prev = match db.get_setting("app")? {
        Some(json) => serde_json::from_str::<AppSettings>(&json).unwrap_or_default(),
        None => AppSettings::default(),
    };
    // 显示桌面免疫开关变化：实时应用到所有已打开的桌面组件/分区窗口
    if prev.keep_on_show_desktop != settings.keep_on_show_desktop {
        for w in app.webview_windows().values() {
            if boxwin::is_desktop_widget(w.label()) {
                boxwin::apply_show_desktop_immune(&w, settings.keep_on_show_desktop);
            }
        }
    }
    // 只有「开机自启」真正发生变化时才动注册表。
    // 之前每次保存设置都会调用 reg.exe，导致在设置页连续调节颜色/透明度时疯狂闪黑窗。
    if prev.auto_start != settings.auto_start {
        engine::set_auto_start(settings.auto_start)?;
    }
    // 界面缩放变化：实时应用到所有已打开的窗口（新窗口创建时也会各自应用）
    if (prev.ui_zoom - settings.ui_zoom).abs() > f64::EPSILON {
        let zoom = settings.ui_zoom.clamp(0.5, 3.0);
        for w in app.webview_windows().values() {
            let _ = w.set_zoom(zoom);
            // 固定尺寸窗口（设置 / 搜索）同步放大窗口本身，
            // 可视内容区域（CSS 像素）不变，只是字和控件等比变大
            match w.label() {
                "main" => {
                    let _ = w.set_size(tauri::LogicalSize::new(760.0 * zoom, 680.0 * zoom));
                }
                "search" => {
                    let _ = w.set_size(tauri::LogicalSize::new(540.0 * zoom, 380.0 * zoom));
                }
                _ => {}
            }
        }
    }
    // 鼠标快捷键配置变化：热更新全局钩子的生效绑定表（无需重启）
    if prev.mouse_hotkeys_enabled != settings.mouse_hotkeys_enabled
        || serde_json::to_string(&prev.mouse_hotkeys).unwrap_or_default()
            != serde_json::to_string(&settings.mouse_hotkeys).unwrap_or_default()
    {
        crate::mousehotkey::update_config(
            settings.mouse_hotkeys_enabled,
            settings.mouse_hotkeys.clone(),
        );
    }
    // 设置变化广播：所有打开的分区窗口收到 settings-changed 后重载。
    // 必须总是发（不能只挑开关）：饼型菜单的图标/圆钮尺寸/弹出尺寸都能在
    // 右键菜单和设置页里改，圆钮窗口要实时跟着变。高频重载由前端节流兜住
    //（BoxWindow 对 load() 做 120ms trailing 节流），后端不丢最后一次变更。
    let _ = app.emit("settings-changed", settings.hide_file_extensions);
    let json = serde_json::to_string(&settings).map_err(|e| e.to_string())?;
    db.set_setting("app", &json)
}

#[tauri::command]
pub fn get_box_files(box_name: String) -> Result<Vec<DesktopFile>, String> {
    engine::get_box_files(&box_name, false)
}

// ---------- 分区窗口 ----------

#[tauri::command]
pub fn get_box_window_cfg(db: State<Db>, id: i64) -> Result<Option<BoxWindowCfg>, String> {
    db.get_box_window_cfg(id)
}

#[tauri::command]
pub fn save_box_window_cfg(
    app: AppHandle,
    db: State<Db>,
    cfg: BoxWindowCfg,
) -> Result<(), String> {
    let prev = db.get_box_window_cfg(cfg.id)?;
    // **几何字段一律以库里现值为准，不被前端覆盖**：前端 updateCfg 发回的是
    // “加载那一刻”的整份配置，里面的 x/y/w/h 早就过期（圆钮被拖过、或饼型
    // 展开/收回时窗口矩形变过）。整体写回会把后端 persist_cfg 记录的实时位置
    // 顶掉 —— 表现就是摆放好的饼型图标（尤其贴屏幕边缘、展开时有平移的）
    // 重启后圆钮偏移。窗口几何的权威来源只有后端的 Moved/Resized 落盘。
    let id = cfg.id;
    let locked = cfg.locked;
    db.update_box_window_cfg(id, |c| {
        c.icon_size = cfg.icon_size;
        c.bg_color = cfg.bg_color;
        c.bg_opacity = cfg.bg_opacity;
        c.label_color = cfg.label_color;
        c.view_mode = cfg.view_mode;
        c.item_order = cfg.item_order;
        c.gap_x = cfg.gap_x;
        c.gap_y = cfg.gap_y;
        // collapsed / expanded_geom 也不回写：胶囊收起/展开几何由后端
        // set_box_collapsed 统一维护，前端在竞态时刻发回的是旧快照
        c.capsule_hover = cfg.capsule_hover;
        c.radial_style = cfg.radial_style;
        c.radial_show_title = cfg.radial_show_title;
        c.radial_show_names = cfg.radial_show_names;
        c.radial_icon = cfg.radial_icon;
        c.locked = locked;
    })?;
    // 锁定状态变化时同步切换原生窗口的可拉伸边框：
    // 锁定 = 不能拖动（前端挡）+ 不能拉伸（去掉 WS_THICKFRAME，窗口样式这一层）
    if prev.as_ref().map(|c| c.locked) != Some(locked) {
        if let Some(win) = app.get_webview_window(&format!("box-{id}")) {
            let _ = win.set_resizable(!locked);
        }
    }
    Ok(())
}

#[tauri::command]
pub fn set_box_visible(app: AppHandle, id: i64, visible: bool) -> Result<(), String> {
    boxwin::set_box_visible(&app, id, visible)
}

/// 托盘菜单窗口前端的心跳打点：挂载完成 / 每次弹出收到通知时调用。
/// 用于确认 WebView 是否活着 —— hide→show 循环后 WebView2 渲染挂起时，
/// 后端明明 show 了但用户看不到，靠这条日志区分"前端死了"还是"窗口没显示"。
#[tauri::command]
pub fn tray_menu_ping(reason: String) -> Result<(), String> {
    crate::touch_tray_menu_heartbeat();
    crate::log_msg(&format!("tray-menu 前端心跳：{reason}"));
    Ok(())
}

/// 收起托盘菜单（挪屏幕外，窗口常驻）。前端点菜单项 / 按 Esc 后调用——
/// 前端不能再 hide() 自己：hide 会让 WebView 挂起，下次弹出可能不渲染。
#[tauri::command]
pub fn tray_menu_dismiss(app: AppHandle) -> Result<(), String> {
    crate::tray_menu_dismiss(&app);
    Ok(())
}

/// 托盘右键菜单（自绘 Win11 风格窗口，见 TrayMenuWindow.tsx）的动作分发。
/// 原生 HMENU 菜单已移除，菜单项逻辑全部收拢到这里。
///
/// **必须是 `async fn`**：同步命令跑在主线程上，而这些动作都要抢数据库锁
/// （`add` → `next_box_name`，`settings` → `show_main` → `apply_ui_zoom` → `ui_zoom`）。
/// 搜索索引重建期间那把锁会被持十几秒（实测 17s），主线程一旦跟着卡住，
/// Windows 就会把应用判为未响应并**停止向托盘图标投递点击**（ghost 化）——
/// 表现就是"点一次新建分区/设置，菜单就再也弹不出来"。
/// 声明成 async 后命令体在异步运行时的线程上跑，主线程照常泵消息。
#[tauri::command]
pub async fn tray_menu_action(app: AppHandle, db: State<'_, Db>, action: String) -> Result<(), String> {
    // 入口日志：这条出现 = invoke 链路（前端→capability→后端）全通；
    // 用户点了菜单但日志没有这条 = 问题在前端或 capability，不在命令逻辑
    crate::log_msg(&format!("tray-menu 动作：{action}"));
    // Db 内部是 Arc<Mutex<Connection>>，克隆只加引用计数、不复制连接，
    // 正好用来把库句柄带出 State<'_>（async 命令里不能持有跨 await 的借用）。
    let db = db.inner().clone();
    match action.as_str() {
        "toggle" => {
            boxwin::toggle_all_boxes(&app);
            Ok(())
        }
        "add" => {
            let name = crate::next_box_name(&db);
            crate::create_box_and_open(&app, &db, &name, "#15b6c9").map(|_| ())
        }
        "addRadial" => {
            // 托盘直达：新建一个饼型菜单分区并立即显示圆钮。
            // 名字传空 → resolve_box_name 自动取「饼型菜单 N」空号。
            crate::create_box_and_open_typed(&app, &db, "", "#15b6c9", "radial").map(|_| ())
        }
        "settings" => {
            crate::show_main(&app);
            Ok(())
        }
        "organize" => {
            // 与原托盘菜单一致：带出主窗口 + 发预览请求，确认后才真正搬
            crate::show_main(&app);
            app.emit("organize-request", ())
                .map_err(|e| e.to_string())
        }
        "quit" => {
            crate::quit_app(&app);
            Ok(())
        }
        _ => Err(format!("未知的托盘菜单动作：{action}")),
    }
}

/// 切换开机自启（托盘菜单里的开关项）。
#[tauri::command]
pub fn set_auto_start_enabled(enable: bool) -> Result<(), String> {
    engine::set_auto_start(enable)
}

/// 收起 / 展开分区（胶囊模式）。几何保存与恢复见 [`boxwin::set_box_collapsed`]。
#[tauri::command]
pub fn set_box_collapsed(app: AppHandle, id: i64, collapsed: bool) -> Result<(), String> {
    boxwin::set_box_collapsed(&app, id, collapsed)
}

/// 吸附候选矩形 + 阈值：其它分区 / 其它组件 / 屏幕工作区，物理像素。
/// 前端拖动（磁性）与拉伸（边缘贴齐）都用它，owner 是自己的窗口 label。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxSnapTargets {
    pub rects: Vec<[i32; 4]>,
    pub threshold: i32,
}

#[tauri::command]
pub fn get_box_snap_targets(app: AppHandle, owner: String) -> BoxSnapTargets {
    let t = boxwin::snap_targets(&app, &owner);
    boxwin::log_snap_targets(&owner, &t);
    BoxSnapTargets {
        rects: t.rects,
        threshold: t.threshold,
    }
}

#[tauri::command]
pub fn start_box_drag(app: AppHandle, box_id: i64, force_moved: bool) -> Result<(), String> {
    boxwin::start_box_drag(app, box_id, force_moved)
}

#[tauri::command]
pub fn install_shell_menu() -> Result<(), String> {
    boxwin::install_shell_menu()
}

#[tauri::command]
pub fn uninstall_shell_menu() -> Result<(), String> {
    boxwin::uninstall_shell_menu()
}

#[tauri::command]
pub fn is_shell_menu_installed() -> Result<bool, String> {
    Ok(boxwin::is_shell_menu_installed())
}

// ---------- 快捷方式小箭头 ----------

#[tauri::command]
pub fn get_shortcut_arrow_hidden() -> Result<bool, String> {
    Ok(crate::shellicon::is_arrow_hidden())
}

/// 分区窗口里的图标是后端缓存过的，且带小箭头叠加：系统设置一变就必须清缓存 +
/// 通知各分区窗口重取，否则会出现"桌面上的箭头没了、分区里还有"的错觉。
fn refresh_box_icons(app: &AppHandle) {
    crate::icons::clear_cache();
    let bw = app.state::<boxwin::BoxWindows>();
    for id in bw.ids() {
        if let Some(label) = bw.label_for(id) {
            if let Some(win) = app.get_webview_window(&label) {
                let _ = win.emit("box-files-changed", id);
            }
        }
    }
}

/// 开关快捷方式小箭头：写系统级注册表后就地重建图标缓存刷新，**不重启资源管理器**。
/// 个别位置若仍是旧箭头，用 apply_shortcut_arrow_forced 强制重启一次。
#[tauri::command]
pub async fn set_shortcut_arrow_hidden(app: AppHandle, hide: bool) -> Result<(), String> {
    crate::shellicon::set_arrow_hidden(&app, hide)?;
    refresh_box_icons(&app);
    Ok(())
}

/// 强制生效：再走一遍注册表 + 重建缓存 + 重启资源管理器，返回结果说明文字。
/// 主要用于上一次 UAC 被取消后重试。
#[tauri::command]
pub async fn apply_shortcut_arrow_forced(
    app: AppHandle,
    hide: bool,
) -> Result<String, String> {
    let msg = crate::shellicon::force_apply(&app, hide);
    refresh_box_icons(&app);
    msg
}

// ---------- 原生 Shell：打开文件 / 系统右键菜单 ----------

/// 用系统默认程序打开文件 / 文件夹（等价于双击）。
/// 不走 opener 插件 —— 插件命令受 Tauri 权限（ACL）管控，缺 `allow-open-path`
/// 会被直接拒绝，前端只看到"双击没反应"。
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    crate::shellmenu::open_path(&path)
}

// ---------- 快速搜索 ----------

/// 搜索结果条目（纯文本，不含图标，便于前端快速渲染）。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub ext: String,
    pub size: i64,
    /// 修改时间（Unix 秒；0 = 未知）
    pub mtime: i64,
}

/// 读取剪贴板内容并分类（搜索面板「粘贴」按钮用）。
///
/// 返回 `text`（有文字）/ `files`（只有文件）/ `empty`（都没有）三类之一。
/// **永远返回 `Ok`** —— 剪贴板空着 / 只有文件 / 被别的程序占用都只是"没内容"，
/// 不该变成一个弹在用户面前的报错框（见 `crate::clipboard`）。
///
/// 注意：这里只做"读"。**不搬任何文件** —— 搜索框是搜索框，不是放文件的地方；
/// 想把剪贴板里的文件放进分区，请直接拖到分区面板上。
#[tauri::command]
pub fn read_clipboard() -> Result<crate::clipboard::ClipboardInfo, String> {
    Ok(crate::clipboard::info())
}

/// 按文件名模糊搜索本地索引（FTS5 trigram）。前缀匹配优先，文件夹略靠前，短名优先。
///
/// 查询串**原样**交给 `db::search_files`：转义、以及「≥3 字符走 trigram 索引 /
/// <3 字符走 LIKE 全表扫描兜底」的分流都在 db 层做（中文 1~2 字词在 trigram 索引里恒不命中）。
#[tauri::command]
pub fn search_files(
    db: State<Db>,
    query: String,
    limit: Option<i32>,
    scopes: Option<Vec<String>>,
    exts: Option<Vec<String>>,
    exclude_exts: Option<Vec<String>>,
    exclude_scopes: Option<Vec<String>>,
) -> Result<Vec<SearchItem>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let scopes = scopes.unwrap_or_else(|| vec!["common".to_string()]);
    // 后缀归一化：去前导点 + 小写（索引里存的就是这种形态）
    let exts = exts.map(|v| {
        v.into_iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect::<Vec<String>>()
    });
    let exclude_exts = exclude_exts.map(|v| {
        v.into_iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect::<Vec<String>>()
    });
    // 排除盘符 → 路径前缀排除（"C" → "C:%"）：连「常用」等混合 scope 里
    // 落在该盘的文件（桌面/文档/下载都在 C 盘）也一并排除
    let exclude_prefixes: Vec<String> = exclude_scopes
        .unwrap_or_default()
        .into_iter()
        .filter_map(|s| {
            let letter = s.trim().trim_end_matches(':').to_ascii_uppercase();
            let mut ch = letter.chars();
            match (ch.next(), ch.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Some(format!("{}:%", c)),
                _ => None,
            }
        })
        .collect();
    let exclude_prefixes = if exclude_prefixes.is_empty() {
        None
    } else {
        Some(exclude_prefixes)
    };
    // 排序方式来自设置页「搜索结果排序」（缺省/旧配置 = 按修改时间倒序）
    let search_sort = db
        .get_setting("app")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.get("searchSort")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "mtime".into());
    let rows = db.search_files(
        &scopes,
        q,
        limit,
        exts.as_deref(),
        exclude_exts.as_deref(),
        exclude_prefixes.as_deref(),
        &search_sort,
    )?;
    Ok(rows
        .into_iter()
        .map(|(path, name, dir, ext, size, mtime)| SearchItem {
            name,
            path,
            is_dir: dir == 1,
            ext,
            size,
            mtime,
        })
        .collect())
}

/// 重建搜索索引（常用目录必做；`full` 为 true 时再扫全盘）。
/// 扫描放到后台线程执行，立即返回 `(0, 0)` 表示「已在后台开始」，
/// 避免全盘递归扫描（可能几分钟）阻塞命令线程、卡住前端。
#[tauri::command]
pub fn rebuild_search_index(
    app: AppHandle,
    full: Option<bool>,
    drive: Option<String>,
) -> Result<(usize, u64), String> {
    let db = app.state::<Db>().inner().clone();
    std::thread::spawn(move || {
        if let Some(d) = drive {
            let (n, s) = crate::search::rebuild_drive(&db, &d);
            crate::log_msg(&format!("盘 {} 索引完成：{} 项，{}s", d, n, s));
        } else {
            let full = full.unwrap_or(false);
            let (n, s) = crate::search::rebuild_index(&db, full);
            crate::log_msg(&format!("搜索索引构建完成：{} 项，{}s", n, s));
        }
    });
    Ok((0, 0))
}

/// 清空某个作用域（盘符）的索引，用于取消勾选某盘。
#[tauri::command]
pub fn clear_index(app: AppHandle, scope: String) -> Result<(), String> {
    let db = app.state::<Db>().inner().clone();
    db.rebuild_file_index(&scope, &[])
        .map_err(|e| format!("清空索引失败: {e}"))
}

/// 列出所有可读盘符（大写，如 "C"）。
#[tauri::command]
pub fn list_drives() -> Vec<String> {
    crate::search::drive_letters()
}

/// 进入「按一下鼠标识别信号」模式：
/// 6 秒内按下的中键 / 侧键通过 `mouse-captured` 事件回传，
/// Esc 取消（`mouse-capture-cancel` 事件）。左/右键不识别、照常放行。
#[tauri::command]
pub fn start_mouse_capture() {
    crate::mousehotkey::begin_capture();
}

/// 在光标处弹出该文件的系统右键菜单（和桌面 / 资源管理器的右键菜单一致）。
///
/// 返回 `"rename"` 表示用户点了我们补进菜单的「重命名」——资源管理器的原生菜单里
/// 本来没有这一项（重命名由资源管理器视图自己实现），所以得由前端就地改名；
/// 其余情况返回 `null`（系统命令已在菜单内部执行完）。
#[tauri::command]
pub fn show_file_menu(app: AppHandle, window: tauri::Window, path: String) -> Result<Option<String>, String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let res = crate::shellmenu::show_context_menu(hwnd, &path);
    // 菜单期间宿主被临时抬到 TOPMOST 档（见 track_menu_at_cursor），现在归位：
    // 重新钉回桌面并沉到"桌面之上、其它窗口之下"
    if let Some(win) = app.get_webview_window(window.label()) {
        boxwin::sink_desktop_widget(&win, &app);
    }
    res
}

/// 在光标处弹出分区空白处的系统右键菜单（和资源管理器里打开文件夹后在空白处右键一致：
/// 粘贴 / 新建 / 查看 / 排序方式 / 刷新 / 属性 …）。
///
/// 分区对应的文件夹：`folder` 类型用 `link_path`，其余类型用桌面下的分区隐藏文件夹。
#[tauri::command]
pub fn show_box_background_menu(
    app: AppHandle,
    window: tauri::Window,
    box_id: i64,
) -> Result<(), String> {
    let db = app.state::<Db>().inner().clone();
    let dir = engine::box_dir_for(&db, box_id)?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    let res = crate::shellmenu::show_folder_background_menu(hwnd, &dir.to_string_lossy());
    // 同 show_file_menu：菜单期间宿主临时置顶，菜单关闭后沉回桌面层
    if let Some(win) = app.get_webview_window(window.label()) {
        boxwin::sink_desktop_widget(&win, &app);
    }
    res
}

/// 就地重命名（同一目录内改名），成功返回新路径。
///
/// 只接受**纯文件名**：带 `\ / : * ? " < > |` 一律拒绝，免得前端传个 `..\x`
/// 把文件挪到别的目录去。同名文件已存在时直接报错，不覆盖。
#[tauri::command]
pub fn rename_file(app: AppHandle, path: String, new_name: String) -> Result<String, String> {
    let src = std::path::Path::new(&path);
    let name = new_name.trim();
    if name.is_empty() {
        return Err("名称不能为空".to_string());
    }
    if name.contains(|c: char| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err("名称里不能包含 \\ / : * ? \" < > |".to_string());
    }
    let parent = src.parent().ok_or("无法定位文件所在文件夹")?;
    let dst = parent.join(name);
    if src == dst.as_path() {
        // 名字没变，直接当成功
        return Ok(path);
    }
    if dst.exists() {
        return Err(format!("「{name}」已存在"));
    }
    std::fs::rename(src, &dst).map_err(|e| format!("重命名失败：{e}"))?;
    // 分区里显示的是真实文件名 → 通知各分区重读列表
    notify_box_files_changed(&app);
    Ok(dst.to_string_lossy().into_owned())
}

// ---------- 分区弹出菜单（独立置顶窗口） ----------

/// 分区菜单失焦看门狗。
///
/// 分区和组件都是桌面的子窗口，点击它们**不一定会把前台从菜单窗口挪走**，
/// 菜单的 blur 事件可能永远不来 → 菜单一直挂在桌面上。这里轮询前台窗口：
/// - 前台是菜单自己 → 保持；
/// - 前台是本进程的系统对话框（原生取色器等，窗口类 #32770）→ 保持；
/// - 其余（点了桌面 / 其它分区组件 / 别的程序）→ 收起菜单。
fn spawn_menu_watchdog(app: &AppHandle, label: String) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
    };

    fn fg_is_dialog_keep(fg: HWND, my_pid: u32) -> bool {
        if fg.is_invalid() {
            return true; // 拿不到前台就不动
        }
        let mut pid = 0u32;
        unsafe {
            let _ = GetWindowThreadProcessId(fg, Some(&mut pid));
        }
        if pid != my_pid {
            return false; // 别的进程 → 收起
        }
        // 本进程：只有系统对话框（原生取色器 / JS 弹窗）才让菜单存活
        let mut buf = [0u16; 16];
        let n = unsafe { GetClassNameW(fg, &mut buf) };
        let cls = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        cls == "#32770"
    }

    let app = app.clone();
    std::thread::spawn(move || {
        // 先等菜单真正成为前台（最多 ~2s；一直等不到说明用户已经点到别处）
        let mut focused = false;
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Some(win) = app.get_webview_window(&label) else {
                return;
            };
            if !win.is_visible().unwrap_or(false) {
                return;
            }
            let Ok(h) = win.hwnd() else { return };
            if unsafe { GetForegroundWindow() } == h {
                focused = true;
                break;
            }
        }
        if !focused {
            return;
        }
        // 前台一旦离开菜单（间隔 150ms 两次确认，避开瞬时抖动）→ 收起
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Some(win) = app.get_webview_window(&label) else {
                return;
            };
            if !win.is_visible().unwrap_or(false) {
                return;
            }
            let Ok(h) = win.hwnd() else { return };
            let fg = unsafe { GetForegroundWindow() };
            if fg == h || fg_is_dialog_keep(fg, std::process::id()) {
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
            let Some(win) = app.get_webview_window(&label) else {
                return;
            };
            if !win.is_visible().unwrap_or(false) {
                return;
            }
            let Ok(h2) = win.hwnd() else { return };
            let fg2 = unsafe { GetForegroundWindow() };
            if fg2 == h2 || fg_is_dialog_keep(fg2, std::process::id()) {
                continue;
            }
            // 点桌面收起分区 = 用户主动隐藏：必须走 hide_win 放行，否则会被
            // 「显示桌面免疫」的兜底补显示立刻弹回来
            boxwin::hide_win(&win);
            return;
        }
    });
}

/// 打开 / 切换分区菜单。
///
/// 菜单是独立置顶小窗口：分区再小也裁不到它，也不会被"沉到桌面层"的分区挡住。
#[tauri::command]
pub async fn open_box_menu(app: AppHandle, box_id: i64) -> Result<(), String> {
    let label = boxwin::menu_label(box_id);
    if let Some(win) = app.get_webview_window(&label) {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            // 再次展开时激活它，这样点到别处会失焦、自动收起
            let _ = win.set_focus();
        }
        return Ok(());
    }
    let win = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?boxmenu={box_id}").into()),
    )
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .inner_size(206.0, 320.0)
    .visible(false)
    .build()
    .map_err(|e| format!("创建分区菜单失败：{e}"))?;
    // 透明无边框窗口在任务栏可能要再强制一次
    boxwin::force_no_taskbar(&win);
    Ok(())
}

/// 菜单量好自身内容高度后调用：按分区位置摆正 + 定尺寸 + 显示。
#[tauri::command]
pub async fn fit_box_menu(app: AppHandle, box_id: i64, height: f64) -> Result<(), String> {
    let win = app
        .get_webview_window(&boxwin::menu_label(box_id))
        .ok_or("菜单窗口不存在")?;
    boxwin::place_box_menu(&app, box_id, &win, height)?;
    win.show().map_err(|e| e.to_string())?;
    // 失焦看门狗：点了桌面 / 其它分区（不一定触发 blur）也能自动收起菜单
    spawn_menu_watchdog(&app, boxwin::menu_label(box_id));
    // 这里不主动 set_focus：前端 fit 完会自己激活一次，避免"激活→重新定位→再激活"打转
    Ok(())
}

/// 收起分区菜单。
#[tauri::command]
pub async fn close_box_menu(app: AppHandle, box_id: i64) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(&boxwin::menu_label(box_id)) {
        let _ = win.hide();
    }
    Ok(())
}

/// 菜单里点选的操作：转交给对应分区窗口执行（菜单本身不带业务逻辑），再收起菜单。
/// `close = false` 用于"改完配置刷新一下、菜单先不收起"的动作。
#[tauri::command]
pub async fn box_menu_action(
    app: AppHandle,
    box_id: i64,
    action: serde_json::Value,
    close: bool,
) -> Result<(), String> {
    if close {
        if let Some(win) = app.get_webview_window(&boxwin::menu_label(box_id)) {
            let _ = win.hide();
        }
    }
    let Some(label) = app.state::<boxwin::BoxWindows>().label_for(box_id) else {
        return Ok(());
    };
    // 必须把 box_id 一起塞进 payload 让分区窗口自己过滤。
    //
    // 原因（踩过一次的坑）：JS 侧 `listen()` 不带 options 时注册的是 `Any` 目标，
    // 而 tauri 的 match_any_or_filter 是
    //     `*target == EventTarget::Any || filter(...)`
    // —— 只要**监听者**是 Any，emit_to 的定向过滤就**完全失效**，发给 box-3 的
    // 事件照样会进 box-1/box-2 的监听器。表现就是"改一个分区的图标大小，
    // 所有分区的图标一起变"，而且吸附/删除这类操作也会在每个分区各弹一次。
    let payload = serde_json::json!({ "boxId": box_id, "action": action });
    let _ = app.emit_to(&label, "box-menu-action", payload);
    Ok(())
}

// ---------------------------------------------------------------------------
// 组件（便签 / 股票 / 黄历 / 诗词）右键菜单：独立置顶小窗口
// ---------------------------------------------------------------------------

/// 组件菜单窗口的 label
pub(crate) fn widget_menu_label(kind: &str) -> String {
    format!("wmenu-{kind}")
}

/// 三个事件名（前端 `widgetMenu.ts` 里也各有一份，改的时候两边一起改）
pub(crate) const EVT_MENU_REQUEST: &str = "widget-menu-request";
pub(crate) const EVT_MENU_MODEL: &str = "widget-menu-model";
pub(crate) const EVT_MENU_PICK: &str = "widget-menu-pick";

/// 各组件最近一次发布的菜单内容（kind → model）。
///
/// 为什么不让两个窗口直接 emit 对问：菜单窗口是"要用时才建"的，建好后
/// 靠事件问组件"你的菜单长什么样"，很容易问空（组件那一刻还没监听上、
/// 或菜单窗自己还没 listen 好）→ 菜单收到空内容 → 只有一条白条。
/// 后端存一份，菜单窗首屏直接 `invoke` 取，拿到就有内容，不会先闪一下空的。
static WIDGET_MENU_MODELS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>,
> = std::sync::OnceLock::new();

fn widget_menu_store(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>> {
    WIDGET_MENU_MODELS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// 组件发布菜单内容：后端留一份 + 广播（菜单窗已经开着时就能实时刷新）
#[tauri::command]
pub async fn widget_menu_publish(
    app: AppHandle,
    kind: String,
    model: serde_json::Value,
) -> Result<(), String> {
    if let Ok(mut m) = widget_menu_store().lock() {
        m.insert(kind.clone(), model.clone());
    }
    let _ = app.emit(
        EVT_MENU_MODEL,
        serde_json::json!({ "kind": kind, "model": model }),
    );
    Ok(())
}

/// 菜单窗口首屏取内容（`invoke` 直取，不依赖事件时序）
#[tauri::command]
pub async fn widget_menu_get(kind: String) -> Result<serde_json::Value, String> {
    let m = widget_menu_store().lock().map_err(|e| e.to_string())?;
    Ok(m.get(&kind)
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new())))
}

/// 菜单窗口索要最新内容：定向发给组件窗口（组件收到后回一次 publish）
#[tauri::command]
pub async fn widget_menu_request(app: AppHandle, kind: String) -> Result<(), String> {
    let Some(owner) = widget_owner_label(&kind) else {
        return Err(format!("未知组件：{kind}"));
    };
    let _ = app.emit_to(owner, EVT_MENU_REQUEST, serde_json::json!({ "kind": kind }));
    Ok(())
}

/// 菜单里点了一项：定向转交给组件窗口执行（菜单本身不带业务逻辑）
#[tauri::command]
pub async fn widget_menu_pick(
    app: AppHandle,
    kind: String,
    id: String,
    value: Option<serde_json::Value>,
) -> Result<(), String> {
    let Some(owner) = widget_owner_label(&kind) else {
        return Err(format!("未知组件：{kind}"));
    };
    let _ = app.emit_to(
        owner,
        EVT_MENU_PICK,
        serde_json::json!({ "kind": kind, "id": id, "value": value }),
    );
    Ok(())
}

/// 组件 kind → 组件自己的窗口 label
pub(crate) fn widget_owner_label(kind: &str) -> Option<&'static str> {
    match kind {
        "notes" => Some("notes"),
        "stock" => Some("stocks"),
        "huangli" => Some("huangli"),
        "poetry" => Some("poetry"),
        "huangdi" => Some("huangdi"),
        "shanghan" => Some("shanghan"),
        _ => None,
    }
}

/// 打开 / 切换组件菜单。
///
/// 和分区菜单同一套做法：菜单画在自己那个置顶小窗口里，
/// 组件再矮也裁不到它（以前画在组件内部，高度不够时下面的项根本看不见）。
/// 菜单只负责渲染，点选通过事件转交给组件窗口自己执行 —— 业务逻辑不搬家。
#[tauri::command]
pub async fn open_widget_menu(app: AppHandle, kind: String) -> Result<(), String> {
    if widget_owner_label(&kind).is_none() {
        return Err(format!("未知组件：{kind}"));
    }
    let label = widget_menu_label(&kind);
    if let Some(win) = app.get_webview_window(&label) {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
        return Ok(());
    }
    let win = tauri::WebviewWindowBuilder::new(
        &app,
        &label,
        tauri::WebviewUrl::App(format!("index.html?widgetmenu={kind}").into()),
    )
    .title("菜单")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .inner_size(206.0, 320.0)
    .visible(false)
    .build()
    .map_err(|e| format!("创建组件菜单失败：{e}"))?;
    boxwin::force_no_taskbar(&win);
    Ok(())
}

/// 菜单量好自身内容高度后调用：贴到组件外侧 + 定尺寸 + 显示。
#[tauri::command]
pub async fn fit_widget_menu(
    app: AppHandle,
    kind: String,
    height: f64,
) -> Result<(), String> {
    let label = widget_menu_label(&kind);
    let win = app.get_webview_window(&label).ok_or("菜单窗口不存在")?;
    place_widget_menu(&app, &kind, &win, height)?;
    win.show().map_err(|e| e.to_string())?;
    spawn_menu_watchdog(&app, label);
    Ok(())
}

/// 收起组件菜单。
#[tauri::command]
pub async fn close_widget_menu(app: AppHandle, kind: String) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(&widget_menu_label(&kind)) {
        let _ = win.hide();
    }
    Ok(())
}

/// 把组件菜单摆到组件右上角外侧（放不下就往回收，绝不跑出屏幕）。
fn place_widget_menu(
    app: &AppHandle,
    kind: &str,
    menu: &tauri::WebviewWindow,
    height: f64,
) -> Result<(), String> {
    // 拿不到组件窗口（极端情况：它刚被销毁）也别让菜单消失 —— 退到鼠标位置摆，
    // 总比 fit 直接失败、菜单永远不显示强。
    let anchor = app
        .get_webview_window(widget_owner_label(kind).ok_or("未知组件")?)
        .and_then(|o| {
            let scale = o.scale_factor().unwrap_or(1.0);
            let pos = o.outer_position().ok()?;
            let size = o.outer_size().ok()?;
            Some((scale, pos, size))
        });
    let zoom = crate::ui_zoom(app);
    let (scale, pos, size) = match anchor {
        Some(a) => a,
        None => {
            use windows::Win32::Foundation::POINT;
            use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
            let mut p = POINT::default();
            let (px, py) = unsafe {
                if GetCursorPos(&mut p).is_ok() {
                    (p.x, p.y)
                } else {
                    (100, 100)
                }
            };
            (
                1.0,
                tauri::PhysicalPosition::new(px, py),
                tauri::PhysicalSize::new(0u32, 0u32),
            )
        }
    };

    // 与 CSS 里 `.box-win-menu` 的宽度保持一致；物理像素 = CSS × 显示器缩放 × 界面缩放
    let w = (206.0 * scale * zoom).round().max(1.0) as i32;
    let h = (height * scale * zoom).round().max(1.0) as i32;
    let mut x = pos.x + size.width as i32 - w;
    let mut y = pos.y + (34.0 * scale * zoom).round() as i32;

    let mon = app
        .get_webview_window(widget_owner_label(kind).unwrap_or(""))
        .and_then(|o| o.current_monitor().ok().flatten())
        .or_else(|| menu.current_monitor().ok().flatten());

    if let Some(mon) = mon {
        let mp = mon.position();
        let ms = mon.size();
        let (left, top) = (mp.x, mp.y);
        let (right, bottom) = (mp.x + ms.width as i32, mp.y + ms.height as i32);
        if y + h > bottom {
            y = (bottom - h).max(top);
        }
        if x + w > right {
            x = right - w;
        }
        x = x.max(left);
        y = y.max(top);
    }

    let _ = menu.set_zoom(zoom);
    menu.set_size(tauri::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32))
        .map_err(|e| e.to_string())?;
    menu.set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 通知每个分区窗口重新读取自己的文件列表（定向发送，避免广播串台）
pub(crate) fn notify_box_files_changed(app: &AppHandle) {
    let bw = app.state::<boxwin::BoxWindows>();
    for id in bw.ids() {
        if let Some(label) = bw.label_for(id) {
            let _ = app.emit_to(&label, "box-files-changed", id);
        }
    }
}

/// 用系统原生拖拽拖出一个文件 / 文件夹（前端按住图标拖动超过阈值时调用）。
///
/// 走 OLE 拖放，拖的是真文件：可以落到桌面、资源管理器窗口、别的分区，
/// 甚至别的程序里。命令在用户**松开鼠标后**才返回，前端据此结束拖拽手势。
///
/// **必须保持同步**（不要加 `async`）：Tauri 里同步命令跑在主线程，而主线程正是
/// 这些窗口的归属线程、也带消息泵 —— OLE 拖放是模态循环，要接管本线程的消息分发。
/// 写成 `async` 会被丢到 tokio 工作线程，那里既没有消息泵、窗口也不属于它，
/// 表现就是"按住图标拖，什么都没发生"。
///
/// 拖拽结束时做三件事（顺序重要）：
/// 1. 若某个分区窗口已经接过这次拖拽（`TRANSFER_HANDLED`）→ 搬运它自己做完了，
///    这里什么都不用管（后台线程会广播刷新）；
/// 2. 否则若文件还在原地 → 按松手时的光标位置判断是不是落在别的分区上（兜底路径）；
/// 3. 再否则（文件已不在原地）→ 是被拖出去了（桌面 / 资源管理器），
///    删掉它那条"收进来"的记录，免得下次启动又把它收回去。
#[tauri::command]
pub fn start_file_drag(
    app: AppHandle,
    window: tauri::Window,
    path: String,
    box_id: Option<i64>,
) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    boxwin::TRANSFER_HANDLED.store(false, Ordering::SeqCst);
    crate::filedrag::start_drag(hwnd, &path)?;

    // 拖拽已结束：分区窗口的 Drop 处理器是同步跑完的，这里读到的一定是最终状态
    if boxwin::TRANSFER_HANDLED.swap(false, Ordering::SeqCst) {
        return Ok(());
    }

    if std::path::Path::new(&path).exists() {
        // 文件还在原地：可能落到了别的分区，但那个分区没收到 Drop 事件。
        // 按光标位置自己判一次；不是分区（落在资源管理器里做复制）就什么都不做。
        if let Some(dst) = boxwin::box_under_cursor(&app, box_id.unwrap_or(-1)) {
            let db = app.state::<Db>().inner().clone();
            let copy = boxwin::modifier_copy();
            let payload = vec![path.clone()];
            std::thread::spawn(move || {
                // 分区窗口的落点事件有时只是**晚到一步**（它在拖拽循环结束后才派发），
                // 这时两路会各搬一遍，后到的一路找不到源，弹一条"找不到文件"的假错误。
                // 先等一小会儿再看：文件已经被搬走就收工。
                // （`engine::transfer_into_box` 里还有按源路径的去重兜底，双保险。）
                std::thread::sleep(std::time::Duration::from_millis(200));
                if !std::path::Path::new(&path).exists() {
                    crate::log_msg("start_file_drag: 文件已被分区接走，跳过兜底搬运");
                    return;
                }
                let r = engine::transfer_into_box(&db, dst, &payload, copy);
                notify_box_files_changed(&app);
                boxwin::emit_transfer_result(&app, dst, r);
            });
        }
        return Ok(());
    }

    // 原地已经没有这个文件了 → 是被**移出**分区了（不是复制）
    if let Some(db) = app.try_state::<Db>() {
        let _ = db.delete_move_by_to(&path);
    }
    // 只有真的拖出去了才刷新列表。文件还在原地时（复制、或"拖回本分区调位置"）
    // 绝不能刷新 —— 一刷新就会把前端刚调好的图标顺序冲掉。
    notify_box_files_changed(&app);
    Ok(())
}

/// 饼型分区展开/收起的置顶切换：展开时文件必须浮在其它分区之上才点得到
/// （TOPMOST + NOACTIVATE，不抢焦点）；收回后归还桌面层。
#[tauri::command]
pub fn set_radial_floating(app: AppHandle, window: tauri::Window, float: bool) {
    if let Some(w) = app.get_webview_window(window.label()) {
        boxwin::set_radial_floating(&w, float);
    }
}

// ---------- 饼型分区嵌套进普通分区 ----------

/// 可嵌套落点的信息（饼型圆钮拖到普通分区上松手时，前端先问一次）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NestTarget {
    pub id: i64,
    pub name: String,
}

/// `nest_scan_cb` 的枚举上下文：受检分区集合（hwnd → id，已排除圆钮自身与
/// 不可见分区）、光标点、命中的最上层分区。EnumWindows 按 z 序从上到下枚举，
/// 第一个"矩形包含光标点"的受检分区就是用户眼里的落点，找到即停。
#[cfg(target_os = "windows")]
struct NestScanCtx {
    map: Vec<(isize, i64)>,
    px: i32,
    py: i32,
    found: Option<i64>,
}

/// 见 [`NestScanCtx`]。返回 FALSE 提前结束枚举（找到了 / 全部扫完没有也要扫完）。
#[cfg(target_os = "windows")]
unsafe extern "system" fn nest_scan_cb(
    hwnd: windows::Win32::Foundation::HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let ctx = &mut *(lparam.0 as *mut NestScanCtx);
    let key = hwnd.0 as isize;
    let Some(pos) = ctx.map.iter().position(|(k, _)| *k == key) else {
        return true.into();
    };
    let mut rect = windows::Win32::Foundation::RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_ok() {
        // 内缩 2px：贴着分区边缘擦过不算，压进去才算
        if ctx.px >= rect.left + 2
            && ctx.px < rect.right - 2
            && ctx.py >= rect.top + 2
            && ctx.py < rect.bottom - 2
        {
            ctx.found = Some(ctx.map[pos].1);
            return false.into();
        }
    }
    true.into()
}

/// 光标当前位置下有没有"可以收留饼型分区"的宿主（排除 exclude 自身）。
/// 只有普通文件 / 软件分区能当宿主，且宿主自己不能也是嵌套的。
///
/// **判定必须严格等于"圆钮被压在分区上"（光标点 ∈ 宿主窗口矩形）**：
/// 曾用"从光标向 8 个方向按圆钮半径外扩探测"绕开 box_at_point 命中自身的问题，
/// 结果圆钮落在分区**附近**（半个钮身搭到边）也被判成嵌套 —— 用户挪一下圆钮、
/// 甚至手抖一下（moved 阈值才 2px），圆钮就悄悄钻进旁边的分区"消失"了。
/// 现在用 EnumWindows 按 z 序找第一个矩形包含光标点的分区窗口（圆钮不在受检
/// 集合里，不构成遮挡），"指着谁就是谁"的语义与 box_at_point 完全一致。
#[tauri::command]
pub fn find_nest_target(app: AppHandle, db: State<Db>, exclude: i64) -> Result<Option<NestTarget>, String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetCursorPos};
        // 受检分区：除圆钮自己外的所有**可见**分区窗口（hwnd → id）
        let bw = app.state::<boxwin::BoxWindows>();
        let mut map: Vec<(isize, i64)> = Vec::new();
        for id in bw.ids() {
            if id == exclude {
                continue;
            }
            let Some(label) = bw.label_for(id) else { continue };
            let Some(win) = app.get_webview_window(&label) else { continue };
            if !win.is_visible().unwrap_or(false) {
                continue;
            }
            if let Ok(h) = win.hwnd() {
                map.push((h.0 as isize, id));
            }
        }
        if map.is_empty() {
            return Ok(None);
        }
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return Ok(None);
        }
        let mut ctx = NestScanCtx {
            map,
            px: pt.x,
            py: pt.y,
            found: None,
        };
        unsafe {
            let _ = EnumWindows(
                Some(nest_scan_cb),
                windows::Win32::Foundation::LPARAM(&mut ctx as *mut NestScanCtx as isize),
            );
        }
        let Some(id) = ctx.found else {
            return Ok(None);
        };
        // 校验这个落点能不能当宿主（类型 + 自己没被嵌套）；最上层命中不合格
        // 就算没有 —— 指着饼型/嵌套分区不往下"穿透"
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        let row = conn
            .query_row(
                "SELECT name, type, nested_in FROM boxes WHERE id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .ok();
        drop(conn);
        match row {
            Some((name, t, nested_in)) if (t == "file" || t == "software") && nested_in == 0 => {
                Ok(Some(NestTarget { id, name }))
            }
            _ => Ok(None),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, db, exclude);
        Ok(None)
    }
}

/// 把饼型分区嵌进宿主分区：写 nested_in 并隐藏圆钮窗口。
/// 嵌套后它只在宿主的文件列表里以一个条目出现，桌面上不再有圆钮。
#[tauri::command]
pub fn nest_radial_box(app: AppHandle, db: State<Db>, box_id: i64, target: i64) -> Result<(), String> {
    if box_id == target {
        return Err("不能嵌套进自己".to_string());
    }
    // 校验双方类型与状态（同一把锁内读，避免中途变化的竞态）
    let (b_type, b_nested): (String, i64) = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT type, nested_in FROM boxes WHERE id = ?1",
            params![box_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "饼型分区不存在".to_string())?
    };
    if b_type != "radial" {
        return Err("只有饼型菜单分区能嵌套".to_string());
    }
    if b_nested != 0 {
        return Err("该饼型分区已嵌套在其他分区里".to_string());
    }
    let (t_type, t_nested, t_name): (String, i64, String) = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT type, nested_in, name FROM boxes WHERE id = ?1",
            params![target],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "目标分区不存在".to_string())?
    };
    if t_type != "file" && t_type != "software" {
        return Err("只能嵌套进普通文件 / 软件分区".to_string());
    }
    if t_nested != 0 {
        return Err("目标分区自身也是嵌套的，不能再收留".to_string());
    }
    {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE boxes SET nested_in = ?2 WHERE id = ?1",
            params![box_id, target],
        )
        .map_err(|e| e.to_string())?;
    }
    // 隐藏圆钮窗口（走"我方主动隐藏"许可路径，防显示桌面免疫把它救回来）
    boxwin::set_radial_window_hidden(&app, box_id, true)?;
    crate::log_msg(&format!("饼型分区已嵌套进「{t_name}」"));
    // 通知宿主分区刷新文件列表（拼出嵌套条目）
    let bw = app.state::<boxwin::BoxWindows>();
    if let Some(label) = bw.label_for(target) {
        let _ = app.emit_to(&label, "box-files-changed", target);
    }
    // **必须同时通知饼型分区自己的窗口**：它的 collapseRadial 靠自己状态里的
    // nestedIn 决定"收回后藏回宿主"。只通知宿主的话，它自己的状态永远是旧的 ——
    // 解嵌后它仍以为自己是嵌套态，下次收起就把自己藏没了（圆钮凭空消失）。
    if let Some(label) = bw.label_for(box_id) {
        let _ = app.emit_to(&label, "box-files-changed", box_id);
    }
    Ok(())
}

/// 把嵌套的饼型分区移出宿主，恢复成桌面上的独立圆钮。
#[tauri::command]
pub fn unnest_radial_box(app: AppHandle, db: State<Db>, box_id: i64) -> Result<(), String> {
    let parent: i64 = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT nested_in FROM boxes WHERE id = ?1",
            params![box_id],
            |r| r.get(0),
        )
        .map_err(|_| "分区不存在".to_string())?
    };
    if parent == 0 {
        return Ok(());
    }
    {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("UPDATE boxes SET nested_in = 0 WHERE id = ?1", params![box_id])
            .map_err(|e| e.to_string())?;
    }
    // 解嵌后必须是"可见的独立分区"：退出落盘可能已把 visible 写成 false
    let _ = db.update_box_window_cfg(box_id, |c| c.visible = true);
    // 重新显示圆钮（位置就是嵌套前的那一处，cfg 几何一直没动）
    boxwin::set_radial_window_hidden(&app, box_id, false)?;
    crate::log_msg("嵌套的饼型分区已移出到桌面");
    // 通知原宿主刷新（条目消失）
    let bw = app.state::<boxwin::BoxWindows>();
    if let Some(label) = bw.label_for(parent) {
        let _ = app.emit_to(&label, "box-files-changed", parent);
    }
    // 同时通知饼型分区自己的窗口刷新 nestedIn（同 nest_radial_box，防过期状态误隐藏）
    if let Some(label) = bw.label_for(box_id) {
        let _ = app.emit_to(&label, "box-files-changed", box_id);
    }
    Ok(())
}

/// 解散宿主分区时，把里面嵌套的饼型分区全部放回桌面（恢复独立）。
pub(crate) fn unnest_all_in(app: &AppHandle, db: &Db, parent_id: i64) {
    let children: Vec<i64> = {
        let conn = match db.conn.lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut stmt = match conn.prepare("SELECT id FROM boxes WHERE nested_in = ?1") {
            Ok(s) => s,
            Err(_) => return,
        };
        let rows = stmt.query_map(params![parent_id], |r| r.get::<_, i64>(0));
        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => return,
        }
    };
    for id in children {
        let _ = db
            .conn
            .lock()
            .map_err(|e| e.to_string())
            .and_then(|conn| {
                conn.execute("UPDATE boxes SET nested_in = 0 WHERE id = ?1", params![id])
                    .map_err(|e| e.to_string())
            });
        // 放回桌面后必须是可见的独立分区
        let _ = db.update_box_window_cfg(id, |c| c.visible = true);
        let _ = boxwin::set_radial_window_hidden(app, id, false);
        // 通知自己的窗口刷新 nestedIn（防过期状态把它重新藏没）
        let bw = app.state::<boxwin::BoxWindows>();
        if let Some(label) = bw.label_for(id) {
            let _ = app.emit_to(&label, "box-files-changed", id);
        }
        crate::log_msg(&format!("宿主分区解散，饼型分区 #{id} 已放回桌面"));
    }
}

/// 打开嵌套的饼型分区（用户点了宿主分区里的圆钮条目）。
/// `cx/cy` 是条目中心的**物理屏幕坐标**（前端用窗口位置 × 缩放算出来），
/// 圆钮会出现在那里（夹回屏幕内），弹出后由它自己的前端展开。
#[tauri::command]
pub fn show_nested_radial(app: AppHandle, box_id: i64, cx: i32, cy: i32) -> Result<(), String> {
    boxwin::open_nested_radial(&app, box_id, cx, cy)
}

/// 嵌套的饼型分区收回圆钮后重新隐藏（回宿主列表里去，而不是留在桌面）。
///
/// **以数据库为准**：nested_in = 0（已是独立分区）就直接不动。前端饼型窗口
/// 自己状态里的 nestedIn 可能过期（刷新事件丢失/时序差），拿过期值调用到这里
/// 会把一个独立圆钮藏进"永久消失"（hide_win 带许可，守护线程不会救回）——
/// 这正是"圆钮用着用着就不见了"的根因，后端必须兜底。
#[tauri::command]
pub fn hide_nested_radial(app: AppHandle, db: State<Db>, box_id: i64) -> Result<(), String> {
    let nested: i64 = {
        let conn = db.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT nested_in FROM boxes WHERE id = ?1",
            params![box_id],
            |r| r.get(0),
        )
        .map_err(|_| "分区不存在".to_string())?
    };
    if nested == 0 {
        return Ok(());
    }
    boxwin::set_radial_window_hidden(&app, box_id, true)
}

// ---------- 便签组件 ----------

/// 某一页（标签）下的便签
#[tauri::command]
pub fn get_notes(db: State<Db>, tab_id: i64) -> Result<Vec<Note>, String> {
    db.list_notes(tab_id)
}

/// 在指定页新增一条空便签
#[tauri::command]
pub fn add_note(db: State<Db>, tab_id: i64) -> Result<i64, String> {
    db.add_note(tab_id)
}

/// 便签页（标题栏标签栏）列表
#[tauri::command]
pub fn list_note_tabs(db: State<Db>) -> Result<Vec<NoteTab>, String> {
    db.list_note_tabs()
}

/// 新建一页（便签），返回其 id
#[tauri::command]
pub fn add_note_tab(db: State<Db>, name: String) -> Result<i64, String> {
    db.add_note_tab(&name)
}

#[tauri::command]
pub fn rename_note_tab(db: State<Db>, id: i64, name: String) -> Result<(), String> {
    db.rename_note_tab(id, &name)
}

/// 删除一页（连同它下面的便签）；至少会保留一页
#[tauri::command]
pub fn delete_note_tab(db: State<Db>, id: i64) -> Result<(), String> {
    db.delete_note_tab(id)
}

#[tauri::command]
pub fn save_note(db: State<Db>, note: Note) -> Result<(), String> {
    db.update_note(&note)
}

#[tauri::command]
pub fn delete_note(db: State<Db>, id: i64) -> Result<(), String> {
    db.delete_note(id)
}

#[tauri::command]
pub fn get_notes_enabled(db: State<Db>) -> Result<bool, String> {
    Ok(crate::notes::is_enabled(&db))
}

/// 显示/隐藏桌面便签（会创建或隐藏窗口，故为 async 避免阻塞 IPC 线程）
#[tauri::command]
pub async fn set_notes_enabled(
    app: AppHandle,
    db: State<'_, Db>,
    enabled: bool,
) -> Result<(), String> {
    let db = db.inner().clone();
    crate::notes::set_enabled(&app, &db, enabled)?;
    let _ = app.emit("notes-enabled-changed", enabled);
    Ok(())
}

#[tauri::command]
pub fn get_note_window_cfg(db: State<Db>) -> Result<NoteWindowCfg, String> {
    Ok(db.get_note_window_cfg()?.unwrap_or_else(NoteWindowCfg::new))
}

#[tauri::command]
pub fn save_note_window_cfg(db: State<Db>, cfg: NoteWindowCfg) -> Result<(), String> {
    db.set_note_window_cfg(&cfg)
}

#[tauri::command]
pub fn start_note_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    crate::notes::start_note_drag(app, force_moved)
}

// ---------- 股票面板组件 ----------

#[tauri::command]
pub fn get_stocks(db: State<Db>) -> Result<Vec<StockItem>, String> {
    db.list_stocks()
}

/// 添加自选股：先解析代码，再顺手抓一次行情把名称补上
#[tauri::command]
pub async fn add_stock(db: State<'_, Db>, code: String) -> Result<StockItem, String> {
    let db = db.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (secid, norm) = crate::stock::parse_secid(&code)?;
        if db.find_stock_by_secid(&secid)?.is_some() {
            return Err("该股票已在自选列表中".to_string());
        }
        let name = match crate::stock::fetch_name(&secid) {
            Ok(Some(n)) => n,
            // 代码无效：不要把查不到的条目写进列表
            Ok(None) => return Err(format!("没有查到「{code}」，请检查代码是否正确")),
            // 网络异常：先按代码存下，行情恢复后仍能正常显示
            Err(_) => norm.clone(),
        };
        let id = db.add_stock(&norm, &secid, &name)?;
        Ok(StockItem {
            id,
            code: norm,
            secid,
            name,
            sort: 0,
        })
    })
    .await
    .map_err(|e| format!("添加失败：{e}"))?
}

#[tauri::command]
pub fn delete_stock(db: State<Db>, id: i64) -> Result<(), String> {
    db.delete_stock(id)
}

#[tauri::command]
pub fn reorder_stocks(db: State<Db>, ids: Vec<i64>) -> Result<(), String> {
    db.reorder_stocks(&ids)
}

/// 批量拉取实时行情（网络请求放到阻塞线程，避免占用异步运行时的线程）
#[tauri::command]
pub async fn fetch_stocks(secids: Vec<String>) -> Result<Vec<Quote>, String> {
    tauri::async_runtime::spawn_blocking(move || crate::stock::fetch_quotes(&secids))
        .await
        .map_err(|e| format!("行情请求失败：{e}"))?
}

#[tauri::command]
pub fn get_stocks_enabled(db: State<Db>) -> Result<bool, String> {
    Ok(crate::stock::is_enabled(&db))
}

/// 显示/隐藏股票面板（会创建或隐藏窗口，故为 async 避免阻塞 IPC 线程）
#[tauri::command]
pub async fn set_stocks_enabled(
    app: AppHandle,
    db: State<'_, Db>,
    enabled: bool,
) -> Result<(), String> {
    let db = db.inner().clone();
    crate::stock::set_enabled(&app, &db, enabled)
}

#[tauri::command]
pub fn get_stock_window_cfg(db: State<Db>) -> Result<StockWindowCfg, String> {
    Ok(db.get_stock_window_cfg()?.unwrap_or_else(StockWindowCfg::new))
}

#[tauri::command]
pub fn save_stock_window_cfg(db: State<Db>, cfg: StockWindowCfg) -> Result<(), String> {
    db.set_stock_window_cfg(&cfg)
}

#[tauri::command]
pub fn start_stock_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    crate::stock::start_stock_drag(app, force_moved)
}

// ---------- 黄历面板组件 ----------

#[tauri::command]
pub fn get_huangli_enabled(db: State<Db>) -> Result<bool, String> {
    Ok(crate::huangli::is_enabled(&db))
}

/// 显示/隐藏黄历面板（会创建或隐藏窗口，故为 async 避免阻塞 IPC 线程）
#[tauri::command]
pub async fn set_huangli_enabled(
    app: AppHandle,
    db: State<'_, Db>,
    enabled: bool,
) -> Result<(), String> {
    let db = db.inner().clone();
    crate::huangli::set_enabled(&app, &db, enabled)
}

#[tauri::command]
pub fn get_huangli_window_cfg(db: State<Db>) -> Result<HuangliWindowCfg, String> {
    Ok(db
        .get_huangli_window_cfg()?
        .unwrap_or_else(HuangliWindowCfg::new))
}

#[tauri::command]
pub fn save_huangli_window_cfg(db: State<Db>, cfg: HuangliWindowCfg) -> Result<(), String> {
    db.set_huangli_window_cfg(&cfg)
}

#[tauri::command]
pub fn start_huangli_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    crate::huangli::start_huangli_drag(app, force_moved)
}

// ---------- 诗词面板 ----------

/// 诗词面板是否启用（设置页“组件 → 诗词”勾选状态）
#[tauri::command]
pub fn get_poetry_enabled(db: State<Db>) -> Result<bool, String> {
    Ok(crate::poetry::is_enabled(&db))
}

/// 显示/隐藏诗词面板（会创建或隐藏窗口，故为 async 避免阻塞 IPC 线程）
#[tauri::command]
pub async fn set_poetry_enabled(
    app: AppHandle,
    db: State<'_, Db>,
    enabled: bool,
) -> Result<(), String> {
    let db = db.inner().clone();
    crate::poetry::set_enabled(&app, &db, enabled)
}

#[tauri::command]
pub fn get_poetry_window_cfg(db: State<Db>) -> Result<PoetryWindowCfg, String> {
    Ok(db
        .get_poetry_window_cfg()?
        .unwrap_or_else(PoetryWindowCfg::new))
}

#[tauri::command]
pub fn save_poetry_window_cfg(db: State<Db>, cfg: PoetryWindowCfg) -> Result<(), String> {
    db.set_poetry_window_cfg(&cfg)
}

#[tauri::command]
pub fn start_poetry_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    crate::poetry::start_poetry_drag(app, force_moved)
}

// ---------- 黄帝内经面板 ----------

/// 黄帝内经面板是否启用（设置页“组件 → 黄帝内经”勾选状态）
#[tauri::command]
pub fn get_huangdi_enabled(db: State<Db>) -> Result<bool, String> {
    Ok(crate::huangdi::is_enabled(&db))
}

/// 显示/隐藏黄帝内经面板（会创建或隐藏窗口，故为 async 避免阻塞 IPC 线程）
#[tauri::command]
pub async fn set_huangdi_enabled(
    app: AppHandle,
    db: State<'_, Db>,
    enabled: bool,
) -> Result<(), String> {
    let db = db.inner().clone();
    crate::huangdi::set_enabled(&app, &db, enabled)
}

#[tauri::command]
pub fn get_huangdi_window_cfg(db: State<Db>) -> Result<HuangdiWindowCfg, String> {
    Ok(db
        .get_huangdi_window_cfg()?
        .unwrap_or_else(HuangdiWindowCfg::new))
}

#[tauri::command]
pub fn save_huangdi_window_cfg(db: State<Db>, cfg: HuangdiWindowCfg) -> Result<(), String> {
    db.set_huangdi_window_cfg(&cfg)
}

#[tauri::command]
pub fn start_huangdi_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    crate::huangdi::start_huangdi_drag(app, force_moved)
}



/// 伤寒论面板是否启用（设置页“组件 → 伤寒论”勾选状态）
#[tauri::command]
pub fn get_shanghan_enabled(db: State<Db>) -> Result<bool, String> {
    Ok(crate::shanghan::is_enabled(&db))
}

/// 显示/隐藏伤寒论面板（会创建或隐藏窗口，故为 async 避免阻塞 IPC 线程）
#[tauri::command]
pub async fn set_shanghan_enabled(
    app: AppHandle,
    db: State<'_, Db>,
    enabled: bool,
) -> Result<(), String> {
    let db = db.inner().clone();
    crate::shanghan::set_enabled(&app, &db, enabled)
}

#[tauri::command]
pub fn get_shanghan_window_cfg(db: State<Db>) -> Result<ShanghanWindowCfg, String> {
    Ok(db
        .get_shanghan_window_cfg()?
        .unwrap_or_else(ShanghanWindowCfg::new))
}

#[tauri::command]
pub fn save_shanghan_window_cfg(db: State<Db>, cfg: ShanghanWindowCfg) -> Result<(), String> {
    db.set_shanghan_window_cfg(&cfg)
}

#[tauri::command]
pub fn start_shanghan_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    crate::shanghan::start_shanghan_drag(app, force_moved)
}
