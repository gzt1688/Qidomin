use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxWindowCfg {
    pub id: i64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub icon_size: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_bg_color")]
    pub bg_color: String,
    #[serde(default)]
    pub bg_opacity: i32,
    /// 分区内文件名的文字颜色：只有纯白 / 纯黑两态，默认白色（深色壁纸下才看得清）
    #[serde(default = "default_label_color")]
    pub label_color: String,
    /// 显示方式："icon" = 大图标网格；"list" = 竖排列表。旧数据缺省按图标
    #[serde(default = "default_view_mode")]
    pub view_mode: String,
    /// 手工排列的图标顺序（存文件路径）。空 = 按分区当前的排序方式显示。
    /// 放在窗口配置里，不用改表结构。
    #[serde(default)]
    pub item_order: Vec<String>,
    /// 图标网格水平间距（px），默认 6
    #[serde(default = "default_gap_x")]
    pub gap_x: i32,
    /// 图标网格垂直间距（px），默认 6
    #[serde(default = "default_gap_y")]
    pub gap_y: i32,
    /// 胶囊模式：true = 当前收起为小胶囊。收起期间 `w`/`h` 是胶囊尺寸，
    /// 展开前的真实几何存 [`Self::expanded_geom`]。
    #[serde(default)]
    pub collapsed: bool,
    /// 收起为胶囊前的窗口几何 [x, y, w, h]（物理像素），展开时按它恢复。
    #[serde(default)]
    pub expanded_geom: Option<[i32; 4]>,
    /// 悬停胶囊时是否自动临时展开（默认 true；false = 只能点击展开）
    #[serde(default = "default_capsule_hover")]
    pub capsule_hover: bool,
    /// 饼型菜单分区样式："" = 普通分区/胶囊；"pie" = 单击圆钮弹出圆周一圈气泡；
    /// "grid" = 单击弹出方形网格气泡。radial 分区永远处于收起（圆钮）语义，
    /// 展开只是临时放大窗口，配置不落盘展开几何。
    #[serde(default)]
    pub radial_style: String,
    /// 饼型圆钮是否显示标题文字（每分区独立开关；图标优先于文字，默认 true）
    #[serde(default = "default_radial_show_title")]
    pub radial_show_title: bool,
    /// 弹出的文件气泡/卡片是否显示文件名（false = 仅图标，默认 true）
    #[serde(default = "default_radial_show_names")]
    pub radial_show_names: bool,
    /// 饼型圆钮图标（每分区独立）：图片 data URL 或绝对路径。
    /// 空 = 用全局默认图标（app 设置里的 radialIcon）。
    #[serde(default)]
    pub radial_icon: String,
    /// 分区归属显示器的硬件身份（显示输出的 PnP 实例路径）。
    /// 空 = 未绑定（老数据 / 从备份恢复），几何完全按 x/y 绝对坐标处理。
    #[serde(default)]
    pub monitor_key: String,
    /// 分区在归属屏工作区内的比例位置 [rx, ry]（0=贴左/上，1=贴右/下）。
    /// 与 monitor_key 配合：分辨率变了 / 屏幕排列换了也能找回原来的位置。
    #[serde(default)]
    pub rel: Option<[f64; 2]>,
}

fn default_capsule_hover() -> bool {
    true
}

fn default_radial_show_title() -> bool {
    true
}

fn default_radial_show_names() -> bool {
    true
}

fn default_gap_x() -> i32 {
    6
}

fn default_gap_y() -> i32 {
    6
}

fn default_bg_color() -> String {
    "#ffffff".into()
}

pub fn default_label_color() -> String {
    "#ffffff".into()
}

pub fn default_view_mode() -> String {
    "icon".into()
}

impl BoxWindowCfg {
    pub fn new(id: i64) -> Self {
        BoxWindowCfg {
            id,
            x: 60,
            y: 60,
            w: 320,
            h: 260,
            icon_size: 48,
            locked: false,
            visible: true,
            bg_color: default_bg_color(),
            bg_opacity: 0,
            label_color: default_label_color(),
            view_mode: default_view_mode(),
            item_order: Vec::new(),
            gap_x: 6,
            gap_y: 6,
            collapsed: false,
            expanded_geom: None,
            capsule_hover: true,
            radial_style: String::new(),
            radial_show_title: true,
            radial_show_names: true,
            radial_icon: String::new(),
            monitor_key: String::new(),
            rel: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: i64,
    pub content: String,
    /// 提醒时间 "YYYY-MM-DD HH:MM"，None/空 = 不提醒
    pub remind_at: Option<String>,
    /// true = 有闹钟（到点响铃），false = 无闹钟（静默提示）
    pub remind_alarm: bool,
    /// 是否已经提醒过
    pub fired: bool,
    /// 所属便签页（标签栏上的那一页）；老数据缺省 0，迁移时会归到默认页
    #[serde(default)]
    pub tab_id: i64,
    /// 最后一次修改时间 "YYYY-MM-DD HH:MM"（空 = 未知，前端不显示）。
    /// 由前端在正文真正变化时写入；写空串表示"不改动"，避免只是失焦/设提醒就把时间刷新。
    #[serde(default)]
    pub modified_at: String,
}

/// 便签页 = 标题栏标签栏上的一页
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteTab {
    pub id: i64,
    pub name: String,
}

/// 便签窗口位置/外观
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteWindowCfg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_note_bg_color")]
    pub bg_color: String,
    #[serde(default = "default_note_bg_opacity")]
    pub bg_opacity: i32,
    /// 算式自动计算：敲 `6+3=` 自动补成 `6+3=9`。
    /// 老配置里没有这个字段 → 默认开启（serde default）。
    #[serde(default = "default_true")]
    pub calc: bool,
}

fn default_note_bg_color() -> String {
    "#fff6cc".into()
}

fn default_note_bg_opacity() -> i32 {
    94
}

fn default_true() -> bool {
    true
}

impl NoteWindowCfg {
    pub fn new() -> Self {
        NoteWindowCfg {
            x: 120,
            y: 120,
            w: 380,
            h: 340,
            locked: false,
            visible: true,
            bg_color: default_note_bg_color(),
            bg_opacity: default_note_bg_opacity(),
            calc: true,
        }
    }
}

impl Default for NoteWindowCfg {
    fn default() -> Self {
        NoteWindowCfg::new()
    }
}

/// 股票面板窗口位置/外观
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockWindowCfg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_stock_bg_color")]
    pub bg_color: String,
    #[serde(default = "default_stock_bg_opacity")]
    pub bg_opacity: i32,
    /// 自动刷新间隔（秒）
    #[serde(default = "default_stock_interval")]
    pub interval: i32,
}

fn default_stock_bg_color() -> String {
    "#ffffff".into()
}

fn default_stock_bg_opacity() -> i32 {
    96
}

fn default_stock_interval() -> i32 {
    5
}

impl StockWindowCfg {
    pub fn new() -> Self {
        StockWindowCfg {
            x: 200,
            y: 160,
            w: 320,
            h: 300,
            locked: false,
            visible: true,
            bg_color: default_stock_bg_color(),
            bg_opacity: default_stock_bg_opacity(),
            interval: default_stock_interval(),
        }
    }
}

impl Default for StockWindowCfg {
    fn default() -> Self {
        StockWindowCfg::new()
    }
}

/// 自选股条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockItem {
    pub id: i64,
    /// 用户输入的代码，如 "600000"
    pub code: String,
    /// 行情接口用的标识，如 "1.600000"
    pub secid: String,
    pub name: String,
    pub sort: i64,
}

/// 黄历面板窗口位置/外观（内容全部由前端本地推算，这里只有窗口状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HuangliWindowCfg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_huangli_bg_color")]
    pub bg_color: String,
    #[serde(default = "default_huangli_bg_opacity")]
    pub bg_opacity: i32,
}

/// 宣纸米黄，和黄历的观感一致
fn default_huangli_bg_color() -> String {
    "#fffdf3".into()
}

fn default_huangli_bg_opacity() -> i32 {
    96
}

impl HuangliWindowCfg {
    pub fn new() -> Self {
        HuangliWindowCfg {
            x: 300,
            y: 200,
            w: 300,
            h: 470,
            locked: false,
            visible: true,
            bg_color: default_huangli_bg_color(),
            bg_opacity: default_huangli_bg_opacity(),
        }
    }
}

impl Default for HuangliWindowCfg {
    fn default() -> Self {
        HuangliWindowCfg::new()
    }
}

/// 诗词面板窗口位置/外观（内容全部由前端本地内置，这里只有窗口状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoetryWindowCfg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_poetry_bg_color")]
    pub bg_color: String,
    #[serde(default = "default_poetry_bg_opacity")]
    pub bg_opacity: i32,
    /// 是否自动轮播（每隔 rotate_sec 秒换一首）
    #[serde(default)]
    pub auto_rotate: bool,
    /// 自动轮播间隔（秒）
    #[serde(default = "default_poetry_rotate_sec")]
    pub rotate_sec: i32,
}

/// 墨绿宣纸感，配诗词
fn default_poetry_bg_color() -> String {
    "#fbf7ee".into()
}

fn default_poetry_bg_opacity() -> i32 {
    94
}

fn default_poetry_rotate_sec() -> i32 {
    20
}

impl PoetryWindowCfg {
    pub fn new() -> Self {
        PoetryWindowCfg {
            x: 340,
            y: 180,
            w: 320,
            h: 420,
            locked: false,
            visible: true,
            bg_color: default_poetry_bg_color(),
            bg_opacity: default_poetry_bg_opacity(),
            auto_rotate: false,
            rotate_sec: default_poetry_rotate_sec(),
        }
    }
}

impl Default for PoetryWindowCfg {
    fn default() -> Self {
        PoetryWindowCfg::new()
    }
}

/// 黄帝内经面板窗口位置/外观（内容全部由前端本地内置，这里只有窗口状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HuangdiWindowCfg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_huangdi_bg_color")]
    pub bg_color: String,
    #[serde(default = "default_huangdi_bg_opacity")]
    pub bg_opacity: i32,
    /// 是否自动轮播（每隔 rotate_sec 秒随机换一篇经文）
    #[serde(default)]
    pub auto_rotate: bool,
    /// 自动轮播间隔（秒）
    #[serde(default = "default_huangdi_rotate_sec")]
    pub rotate_sec: i32,
}

/// 米白宣纸感，配黄帝内经
fn default_huangdi_bg_color() -> String {
    "#f4efe3".into()
}

fn default_huangdi_bg_opacity() -> i32 {
    95
}

fn default_huangdi_rotate_sec() -> i32 {
    30
}

impl HuangdiWindowCfg {
    pub fn new() -> Self {
        HuangdiWindowCfg {
            x: 380,
            y: 200,
            w: 360,
            h: 460,
            locked: false,
            visible: true,
            bg_color: default_huangdi_bg_color(),
            bg_opacity: default_huangdi_bg_opacity(),
            auto_rotate: false,
            rotate_sec: default_huangdi_rotate_sec(),
        }
    }
}

impl Default for HuangdiWindowCfg {
    fn default() -> Self {
        HuangdiWindowCfg::new()
    }
}

/// 伤寒论面板窗口位置/外观（内容全部由前端本地内置，这里只有窗口状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShanghanWindowCfg {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub locked: bool,
    pub visible: bool,
    #[serde(default = "default_shanghan_bg_color")]
    pub bg_color: String,
    #[serde(default = "default_shanghan_bg_opacity")]
    pub bg_opacity: i32,
    /// 是否自动轮播（每隔 rotate_sec 秒随机换一条条文）
    #[serde(default)]
    pub auto_rotate: bool,
    /// 自动轮播间隔（秒）
    #[serde(default = "default_shanghan_rotate_sec")]
    pub rotate_sec: i32,
}

/// 淡青灰，配伤寒论
fn default_shanghan_bg_color() -> String {
    "#eaf0f2".into()
}

fn default_shanghan_bg_opacity() -> i32 {
    95
}

fn default_shanghan_rotate_sec() -> i32 {
    30
}

impl ShanghanWindowCfg {
    pub fn new() -> Self {
        ShanghanWindowCfg {
            x: 820,
            y: 200,
            w: 360,
            h: 460,
            locked: false,
            visible: true,
            bg_color: default_shanghan_bg_color(),
            bg_opacity: default_shanghan_bg_opacity(),
            auto_rotate: false,
            rotate_sec: default_shanghan_rotate_sec(),
        }
    }
}

impl Default for ShanghanWindowCfg {
    fn default() -> Self {
        ShanghanWindowCfg::new()
    }
}

/// 分区「吸附同类文件」配置：把桌面上这些扩展名的文件收进该分区。
/// `auto` 为真时，后台会持续把新出现的同类文件也吸进来。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AbsorbCfg {
    /// 小写、不含点的扩展名
    pub exts: Vec<String>,
    /// 是否自动吸附新出现的同类文件
    pub auto: bool,
}

#[derive(Clone)]
pub struct Db {
    pub conn: std::sync::Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA temp_store = MEMORY;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 3000;
             CREATE TABLE IF NOT EXISTS boxes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT UNIQUE NOT NULL,
                 color TEXT NOT NULL DEFAULT '#15b6c9',
                 created_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS rules (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL,
                 box_id INTEGER NOT NULL REFERENCES boxes(id) ON DELETE CASCADE,
                 match_type TEXT NOT NULL,
                 pattern TEXT NOT NULL,
                 priority INTEGER NOT NULL DEFAULT 100,
                 enabled INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE IF NOT EXISTS settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS move_history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 file_name TEXT NOT NULL,
                 from_path TEXT NOT NULL,
                 to_path TEXT NOT NULL,
                 moved_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS organize_log (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 batch_id TEXT NOT NULL,
                 from_path TEXT NOT NULL,
                 to_path TEXT NOT NULL,
                 created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_organize_log_batch ON organize_log(batch_id);
             CREATE TABLE IF NOT EXISTS notes (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 content TEXT NOT NULL DEFAULT '',
                 remind_at TEXT,
                 remind_alarm INTEGER NOT NULL DEFAULT 1,
                 fired INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL,
                 modified_at TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS stocks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 code TEXT NOT NULL,
                 secid TEXT NOT NULL,
                 name TEXT NOT NULL DEFAULT '',
                 sort_order INTEGER NOT NULL DEFAULT 0,
                 created_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_rules_box ON rules(box_id);
             CREATE INDEX IF NOT EXISTS idx_notes_remind ON notes(fired, remind_at);
             CREATE INDEX IF NOT EXISTS idx_move_history_to ON move_history(to_path);
             DROP TABLE IF EXISTS files_index;
             CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
                 name,
                 path UNINDEXED,
                 dir UNINDEXED,
                 ext UNINDEXED,
                 size UNINDEXED,
                 mtime UNINDEXED,
                 scope UNINDEXED,
                 tokenize='trigram case_sensitive 0'
             );",
        )
        .map_err(|e| e.to_string())?;
        let db = Db {
            conn: std::sync::Arc::new(Mutex::new(conn)),
        };
        db.migrate()?;
        db.seed_defaults()?;
        Ok(db)
    }

    /// 旧库迁移：为 boxes 表补充类型/链接/排序列
    fn migrate(&self) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cols: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(boxes)")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if !cols.iter().any(|c| c == "type") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN type TEXT NOT NULL DEFAULT 'file'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if !cols.iter().any(|c| c == "link_path") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN link_path TEXT",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if !cols.iter().any(|c| c == "sort_mode") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN sort_mode TEXT NOT NULL DEFAULT 'name'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if !cols.iter().any(|c| c == "absorb_exts") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN absorb_exts TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if !cols.iter().any(|c| c == "absorb_auto") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN absorb_auto INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if !cols.iter().any(|c| c == "sort_desc") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN sort_desc INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        // 饼型菜单分区可以嵌进普通文件/软件分区：nested_in = 宿主分区 id（0 = 独立）。
        // 嵌套后圆钮窗口整体隐藏，只在宿主分区的文件列表里以一个条目出现。
        if !cols.iter().any(|c| c == "nested_in") {
            conn.execute(
                "ALTER TABLE boxes ADD COLUMN nested_in INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        // 关联文件夹改成默认按「修改日期」降序（最新在前）。
        // 只动"从没被改过"的那些（还是内置默认的 名称+升序），用户手动选过的排序不碰。
        conn.execute(
            "UPDATE boxes SET sort_mode = 'time', sort_desc = 1 \
             WHERE type = 'folder' AND link_path IS NOT NULL AND link_path != '' \
               AND sort_mode = 'name' AND sort_desc = 0",
            [],
        )
        .map_err(|e| e.to_string())?;

        // ---------- 便签页（标签）----------
        // 老库只有一个便签本，这里补出"多页"结构：一张 note_tabs 表 + notes.tab_id
        conn.execute(
            "CREATE TABLE IF NOT EXISTS note_tabs (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT NOT NULL,
                 created_at TEXT NOT NULL
             )",
            [],
        )
        .map_err(|e| e.to_string())?;
        let note_cols: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(notes)")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| e.to_string())?;
            rows.filter_map(|r| r.ok()).collect()
        };
        if !note_cols.iter().any(|c| c == "tab_id") {
            conn.execute(
                "ALTER TABLE notes ADD COLUMN tab_id INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        // 修改时间列：老库补上，并用创建时间回填，保证每条便签一开始就有个时间可显示
        if !note_cols.iter().any(|c| c == "modified_at") {
            conn.execute(
                "ALTER TABLE notes ADD COLUMN modified_at TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(|e| e.to_string())?;
            conn.execute(
                "UPDATE notes SET modified_at = created_at WHERE TRIM(modified_at) = ''",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        // 一页都没有（老库首次升级）→ 建个默认页，把已有便签全归进去
        let tab_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM note_tabs", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if tab_count == 0 {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            conn.execute(
                "INSERT INTO note_tabs (name, created_at) VALUES ('便签', ?1)",
                params![now],
            )
            .map_err(|e| e.to_string())?;
            let id = conn.last_insert_rowid();
            conn.execute("UPDATE notes SET tab_id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        // 兜底：tab_id 指向已经不存在的页（那页被删过）→ 归到第一页
        conn.execute(
            "UPDATE notes SET tab_id = (SELECT MIN(id) FROM note_tabs) \
             WHERE tab_id NOT IN (SELECT id FROM note_tabs)",
            [],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 新库默认：3 个类型分区各一个（软件 / 文件 / 文件夹）
    fn seed_defaults(&self) -> Result<(), String> {
        let count: i64 = self
            .conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row("SELECT COUNT(*) FROM boxes", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if count > 0 {
            return Ok(());
        }
        let defaults: [(&str, &str, &str); 3] = [
            ("软件", "#15b6c9", "software"),
            ("文件", "#1a9c5d", "file"),
            ("文件夹", "#5b8def", "folder"),
        ];
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        for (name, color, box_type) in defaults.iter() {
            conn.execute(
                "INSERT INTO boxes (name, color, type, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![name, color, box_type, now],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 查询分区类型/链接路径/排序模式/排序方向
    pub fn box_type_info(
        &self,
        id: i64,
    ) -> Result<Option<(String, Option<String>, String, bool)>, String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT type, link_path, sort_mode, sort_desc FROM boxes WHERE id = ?1",
                params![id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)? != 0,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    /// 所有"关联文件夹"分区：(box_id, link_path)。启动时用来挂上文件监控。
    pub fn list_linked_folders(&self) -> Result<Vec<(i64, String)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, link_path FROM boxes \
                 WHERE type = 'folder' AND link_path IS NOT NULL AND link_path != ''",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows.flatten() {
            out.push(r);
        }
        Ok(out)
    }

    /// 设置 / 更改分区的链接文件夹。
    ///
    /// 关联文件夹默认按**修改日期降序**（最新在前）：这类分区看的通常就是
    /// "最近动过什么"，按名称排基本没用。用户仍可在分区菜单里改成别的排序方式。
    pub fn set_box_link(&self, id: i64, path: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        if path.trim().is_empty() {
            // 取消关联：排序方式保持原样
            conn.execute(
                "UPDATE boxes SET link_path = ?1 WHERE id = ?2",
                params![path, id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            conn.execute(
                "UPDATE boxes SET link_path = ?1, sort_mode = 'time', sort_desc = 1 WHERE id = ?2",
                params![path, id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn set_box_sort(&self, id: i64, sort_mode: &str, sort_desc: bool) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE boxes SET sort_mode = ?1, sort_desc = ?2 WHERE id = ?3",
                params![sort_mode, sort_desc as i64, id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 分区：吸附同类文件 ----------

    /// 读取分区的吸附配置
    pub fn get_absorb_cfg(&self, box_id: i64) -> Result<AbsorbCfg, String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT absorb_exts, absorb_auto FROM boxes WHERE id = ?1",
                params![box_id],
                |r| {
                    let raw: String = r.get(0)?;
                    Ok(AbsorbCfg {
                        exts: raw
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect(),
                        auto: r.get::<_, i64>(1)? != 0,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "分区不存在".to_string())
    }

    /// 保存分区吸附配置（扩展名统一小写、去点、去重）
    pub fn set_absorb_cfg(&self, box_id: i64, exts: &[String], auto: bool) -> Result<(), String> {
        let mut clean: Vec<String> = Vec::new();
        for raw in exts {
            let e = raw.trim().trim_start_matches('.').to_lowercase();
            // 扩展名里不该出现分隔符/路径字符，挡掉脏数据
            if e.is_empty() || e.chars().any(|c| c == ',' || c == '\\' || c == '/') {
                continue;
            }
            if !clean.contains(&e) {
                clean.push(e);
            }
        }
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE boxes SET absorb_exts = ?1, absorb_auto = ?2 WHERE id = ?3",
                params![clean.join(","), if auto { 1 } else { 0 }, box_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 开启了「自动吸附」且已配置类型的非文件夹分区
    pub fn auto_absorb_boxes(&self) -> Result<Vec<i64>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id FROM boxes
                 WHERE absorb_auto = 1 AND absorb_exts != '' AND type != 'folder'
                 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|e| e.to_string())?);
        }
        Ok(ids)
    }

    /// 在**已持有连接锁**的前提下读写设置。
    /// 供"读-改-写"这类需要原子性的操作复用（自己去 lock 会和调用方死锁）。
    fn get_setting_with(conn: &Connection, key: &str) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| e.to_string())
    }

    pub(crate) fn set_setting_with(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        Self::get_setting_with(&conn, key)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        Self::set_setting_with(&conn, key, value)
    }

    pub fn box_names(&self) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT name FROM boxes ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r: &Row| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        let mut names = Vec::new();
        for row in rows {
            names.push(row.map_err(|e| e.to_string())?);
        }
        Ok(names)
    }

    pub fn record_move(&self, file_name: &str, from: &str, to: &str) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "INSERT INTO move_history (file_name, from_path, to_path, moved_at) VALUES (?1, ?2, ?3, ?4)",
                params![file_name, from, to, now],
            );
        }
    }

    /// 删掉"移动到某路径"的记录。
    /// 文件被拖出分区后用得上：不删的话下次启动会按记录又把它收回去。
    pub fn delete_move_by_to(&self, to_path: &str) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "DELETE FROM move_history WHERE to_path = ?1",
                params![to_path],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_move_history(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM move_history WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 整理批次日志（自动整理的撤销依据） ----------

    /// 记一批整理动作。`from`/`to` 都是**精确路径**，撤销时按它们原路搬回。
    pub fn organize_log_insert_batch(
        &self,
        batch_id: &str,
        items: &[(String, String)],
    ) -> Result<(), String> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO organize_log (batch_id, from_path, to_path, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| e.to_string())?;
            for (from, to) in items {
                stmt.execute(params![batch_id, from, to, now])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 最近一次整理批次：(batch_id, 文件数, 时间)。没有任何整理记录时返回 None。
    pub fn latest_organize_batch(&self) -> Result<Option<(String, i64, String)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT batch_id, COUNT(*), MAX(created_at) FROM organize_log
                 GROUP BY batch_id ORDER BY MAX(created_at) DESC, batch_id DESC LIMIT 1",
            )
            .map_err(|e| e.to_string())?;
        let row = stmt
            .query_row([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    /// 取一批整理记录的 (from, to)，按写入顺序返回。
    pub fn organize_batch_items(&self, batch_id: &str) -> Result<Vec<(String, String)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT from_path, to_path FROM organize_log
                 WHERE batch_id = ?1 ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![batch_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|e| e.to_string())?);
        }
        Ok(items)
    }

    /// 批次撤销完成后删掉它（搬不动的条目也一起删：重试只会重复失败）。
    pub fn organize_log_delete_batch(&self, batch_id: &str) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "DELETE FROM organize_log WHERE batch_id = ?1",
                params![batch_id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 删掉所有**落点正好在这个目录里**的移动记录（按父目录精确匹配，不是字符串前缀）。
    ///
    /// 用于「只解散分组」：文件原地交给用户之后，程序不该再按这些记录去搬它，
    /// 否则下次启动会把刚交还给用户的文件又收进隐藏目录。
    /// 返回删掉的条数。
    pub fn delete_move_history_under(&self, dir: &str) -> Result<usize, String> {
        let target = std::path::Path::new(dir);
        let ids: Vec<i64> = self
            .get_move_history()?
            .into_iter()
            .filter(|(_, _, _, to)| std::path::Path::new(to).parent() == Some(target))
            .map(|(id, _, _, _)| id)
            .collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        for id in &ids {
            conn.execute("DELETE FROM move_history WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        Ok(ids.len())
    }

    /// 查「这个路径当初是从哪儿搬过来的」。
    ///
    /// 用在"分区 A 的文件又被搬进分区 B"时：把记录续上，让 `from_path` 始终是最初的
    /// 真实位置（桌面 / 关联文件夹），而不是 A 分区那个隐藏文件夹。
    /// 返回 (记录 id, 最初的原始位置)。
    pub fn move_history_origin(&self, from_path: &str) -> Result<Option<(i64, String)>, String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row(
                // lower() 只对 ASCII 生效，正好够用：Windows 路径不区分大小写的就是 ASCII 部分
                "SELECT id, from_path FROM move_history WHERE lower(to_path) = lower(?1) \
                 ORDER BY id DESC LIMIT 1",
                params![from_path],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    /// 把一条移动记录的落点改到新位置（原始位置保持不变）。
    pub fn update_move_history(
        &self,
        id: i64,
        from: &str,
        to: &str,
        file_name: &str,
    ) -> Result<(), String> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE move_history SET file_name = ?1, from_path = ?2, to_path = ?3, moved_at = ?4 \
                 WHERE id = ?5",
                params![file_name, from, to, now, id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 只改一条记录的**原始位置**（落点不动）。
    ///
    /// 用于修正历史遗留的脏来源：老版本把跨分区搬运记成了 `from = 另一个分区的隐藏目录`，
    /// 不修的话解散时会按它把文件塞回那个用户看不见的隐藏文件夹。
    pub fn set_move_origin(&self, id: i64, from: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE move_history SET from_path = ?1 WHERE id = ?2",
            params![from, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get_move_history(&self) -> Result<Vec<(i64, String, String, String)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, file_name, from_path, to_path FROM move_history ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let list: Vec<(i64, String, String, String)> =
            rows.filter_map(|r| r.ok()).collect();
        Ok(list)
    }

    // ---------- 分区窗口布局持久化 ----------

    pub fn get_box_windows_cfg(&self) -> Result<Vec<BoxWindowCfg>, String> {
        match self.get_setting("boxWindows")? {
            Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string()),
            None => Ok(Vec::new()),
        }
    }

    pub fn get_box_window_cfg(&self, id: i64) -> Result<Option<BoxWindowCfg>, String> {
        Ok(self.get_box_windows_cfg()?.into_iter().find(|c| c.id == id))
    }

    /// 保存某个分区的窗口配置。
    ///
    /// "读列表 → 改一项 → 写回"必须在**同一把锁**里做完：所有分区共用 `boxWindows`
    /// 这一个设置键，如果读和写之间被别的分区插进来（例如另一个分区正在拖动，
    /// 后端 persist_cfg 也在写），就会发生丢更新 —— 表现为"改了 A 分区，B 分区的
    /// 配置莫名其妙退回旧值"。
    pub fn set_box_window_cfg(&self, cfg: &BoxWindowCfg) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut list: Vec<BoxWindowCfg> = match Self::get_setting_with(&conn, "boxWindows")? {
            Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        if let Some(existing) = list.iter_mut().find(|c| c.id == cfg.id) {
            *existing = cfg.clone();
        } else {
            list.push(cfg.clone());
        }
        let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
        Self::set_setting_with(&conn, "boxWindows", &json)
    }

    /// 在**同一把锁**内完成"读列表 → 改一项 → 写回"。
    ///
    /// 这是修改分区窗口配置的**唯一推荐入口**。所有分区共用 `boxWindows` 一个设置键，
    /// 如果读和写分开加锁，中间插进来的其它写者就会被整体覆盖 —— 表现为"改了 A 分区，
    /// B 分区的配置莫名退回旧值"，以及前端刚保存的图标大小/排序被后端拖动落盘冲掉。
    pub fn update_box_window_cfg<F>(&self, id: i64, update: F) -> Result<(), String>
    where
        F: FnOnce(&mut BoxWindowCfg),
    {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut list: Vec<BoxWindowCfg> = match Self::get_setting_with(&conn, "boxWindows")? {
            Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        match list.iter_mut().find(|c| c.id == id) {
            Some(existing) => update(existing),
            None => {
                let mut cfg = BoxWindowCfg::new(id);
                update(&mut cfg);
                list.push(cfg);
            }
        }
        let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
        Self::set_setting_with(&conn, "boxWindows", &json)
    }

    pub fn remove_box_window_cfg(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut list: Vec<BoxWindowCfg> = match Self::get_setting_with(&conn, "boxWindows")? {
            Some(json) => serde_json::from_str(&json).map_err(|e| e.to_string())?,
            None => Vec::new(),
        };
        list.retain(|c| c.id != id);
        let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
        Self::set_setting_with(&conn, "boxWindows", &json)
    }

    // ---------- 便签 ----------

    /// 某一页（标签）下的便签，按创建顺序
    pub fn list_notes(&self, tab_id: i64) -> Result<Vec<Note>, String> {
        self.query_notes(Some(tab_id))
    }

    /// 便签表一行的映射（`query_notes` / `due_notes` 共用）
    fn note_from_row(r: &rusqlite::Row) -> rusqlite::Result<Note> {
        Ok(Note {
            id: r.get(0)?,
            content: r.get(1)?,
            remind_at: r.get::<_, Option<String>>(2)?,
            remind_alarm: r.get::<_, i64>(3)? != 0,
            fired: r.get::<_, i64>(4)? != 0,
            tab_id: r.get(5)?,
            modified_at: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    }

    fn query_notes(&self, tab_id: Option<i64>) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // 固定两条 SQL，避免拼接 where 子句（消除脆弱写法与注入隐患）
        let mut stmt = if tab_id.is_some() {
            conn.prepare(
                "SELECT id, content, remind_at, remind_alarm, fired, tab_id, modified_at \
                 FROM notes WHERE tab_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?
        } else {
            conn.prepare(
                "SELECT id, content, remind_at, remind_alarm, fired, tab_id, modified_at \
                 FROM notes ORDER BY id",
            )
            .map_err(|e| e.to_string())?
        };
        // t_val 必须活到 query_map 结束，故提到外层作用域，避免指向 match 分支内临时量的悬垂引用
        let t_val: i64 = tab_id.unwrap_or(0);
        let params: &[&dyn rusqlite::ToSql] = if tab_id.is_some() {
            &[&t_val as &dyn rusqlite::ToSql]
        } else {
            &[]
        };
        let rows = stmt
            .query_map(params, Self::note_from_row)
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        Ok(list)
    }

    /// 在指定页新建一条空白便签，返回其 id
    pub fn add_note(&self, tab_id: i64) -> Result<i64, String> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO notes (content, remind_alarm, fired, created_at, tab_id, modified_at) \
             VALUES ('', 1, 0, ?1, ?2, ?1)",
            params![now, tab_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    // ---------- 便签页（标签）----------

    pub fn list_note_tabs(&self) -> Result<Vec<NoteTab>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, name FROM note_tabs ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(NoteTab {
                    id: r.get(0)?,
                    name: r.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        Ok(list)
    }

    /// 新建一页，返回其 id
    pub fn add_note_tab(&self, name: &str) -> Result<i64, String> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let name = name.trim();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        // 没传名字时按已有页数顺延：便签1、便签2、便签3……
        let name = if name.is_empty() {
            let cnt: i64 = conn
                .query_row("SELECT COUNT(*) FROM note_tabs", [], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            format!("便签{}", cnt + 1)
        } else {
            name.to_string()
        };
        conn.execute(
            "INSERT INTO note_tabs (name, created_at) VALUES (?1, ?2)",
            params![name, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    pub fn rename_note_tab(&self, id: i64, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("名称不能为空".to_string());
        }
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE note_tabs SET name = ?1 WHERE id = ?2",
                params![name, id],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 删掉一页，连同它下面所有便签。**始终至少留一页**：删光了会自动补一个空的。
    pub fn delete_note_tab(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM notes WHERE tab_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM note_tabs WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM note_tabs", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if left == 0 {
            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            conn.execute(
                "INSERT INTO note_tabs (name, created_at) VALUES ('便签', ?1)",
                params![now],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn update_note(&self, n: &Note) -> Result<(), String> {
        let remind_at = n
            .remind_at
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute(
                "UPDATE notes SET content = ?1, remind_at = ?2, remind_alarm = ?3, fired = ?4 WHERE id = ?5",
                params![
                    n.content,
                    remind_at,
                    if n.remind_alarm { 1 } else { 0 },
                    if n.fired { 1 } else { 0 },
                    n.id
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delete_note(&self, id: i64) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute("DELETE FROM notes WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 到点且尚未提醒过的便签（now 格式 "YYYY-MM-DD HH:MM"，同格式字符串可直接比较）。
    /// 跨所有便签页检查 —— 到点了不管在哪一页都得提醒。
    pub fn due_notes(&self, now: &str) -> Result<Vec<Note>, String> {
        // 在 SQL 层过滤：这个查询每 15 秒跑一次，原来要把所有便签（含正文）
        // 全部读出来再在 Rust 里筛 —— 便签一多就是每 15 秒一次全表扫描 + 全量反序列化。
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, content, remind_at, remind_alarm, fired, tab_id, modified_at \
                 FROM notes \
                 WHERE fired = 0 AND remind_at IS NOT NULL \
                   AND TRIM(remind_at) <> '' AND TRIM(remind_at) <= ?1 \
                 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let q = now.trim();
        let rows = stmt
            .query_map(params![q], Self::note_from_row)
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        Ok(list)
    }

    pub fn mark_note_fired(&self, id: i64) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute("UPDATE notes SET fired = 1 WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---------- 便签窗口配置 ----------

    pub fn get_note_window_cfg(&self) -> Result<Option<NoteWindowCfg>, String> {
        match self.get_setting("noteWindow")? {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    pub fn set_note_window_cfg(&self, cfg: &NoteWindowCfg) -> Result<(), String> {
        let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
        self.set_setting("noteWindow", &json)
    }

    // ---------- 股票自选 ----------

    pub fn list_stocks(&self) -> Result<Vec<StockItem>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, code, secid, name, sort_order FROM stocks ORDER BY sort_order, id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(StockItem {
                    id: r.get(0)?,
                    code: r.get(1)?,
                    secid: r.get(2)?,
                    name: r.get(3)?,
                    sort: r.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut list = Vec::new();
        for row in rows {
            list.push(row.map_err(|e| e.to_string())?);
        }
        Ok(list)
    }

    /// 已存在相同 secid 时返回其 id，不重复添加
    pub fn find_stock_by_secid(&self, secid: &str) -> Result<Option<i64>, String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .query_row(
                "SELECT id FROM stocks WHERE secid = ?1",
                params![secid],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn add_stock(&self, code: &str, secid: &str, name: &str) -> Result<i64, String> {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let next: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM stocks",
                [],
                |r| r.get(0),
            )
            .unwrap_or(1);
        conn.execute(
            "INSERT INTO stocks (code, secid, name, sort_order, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![code, secid, name, next, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    pub fn delete_stock(&self, id: i64) -> Result<(), String> {
        self.conn
            .lock()
            .map_err(|e| e.to_string())?
            .execute("DELETE FROM stocks WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 按给定 id 顺序重排自选股
    pub fn reorder_stocks(&self, ids: &[i64]) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        for (i, id) in ids.iter().enumerate() {
            conn.execute(
                "UPDATE stocks SET sort_order = ?1 WHERE id = ?2",
                params![i as i64, id],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ---------- 股票面板窗口配置 ----------

    pub fn get_stock_window_cfg(&self) -> Result<Option<StockWindowCfg>, String> {
        match self.get_setting("stockWindow")? {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    pub fn set_stock_window_cfg(&self, cfg: &StockWindowCfg) -> Result<(), String> {
        let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
        self.set_setting("stockWindow", &json)
    }

    // ---------- 黄历面板窗口配置 ----------

    pub fn get_huangli_window_cfg(&self) -> Result<Option<HuangliWindowCfg>, String> {
        match self.get_setting("huangliWindow")? {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    pub fn set_huangli_window_cfg(&self, cfg: &HuangliWindowCfg) -> Result<(), String> {
        let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
        self.set_setting("huangliWindow", &json)
    }

    // ---------- 诗词面板窗口配置 ----------

    pub fn get_poetry_window_cfg(&self) -> Result<Option<PoetryWindowCfg>, String> {
        match self.get_setting("poetryWindow")? {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    pub fn set_poetry_window_cfg(&self, cfg: &PoetryWindowCfg) -> Result<(), String> {
        let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
        self.set_setting("poetryWindow", &json)
    }

    // ---------- 黄帝内经面板窗口配置 ----------

    pub fn get_huangdi_window_cfg(&self) -> Result<Option<HuangdiWindowCfg>, String> {
        match self.get_setting("huangdiWindow")? {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    pub fn set_huangdi_window_cfg(&self, cfg: &HuangdiWindowCfg) -> Result<(), String> {
        let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
        self.set_setting("huangdiWindow", &json)
    }


    // ---------- 伤寒论面板窗口配置 ----------

    pub fn get_shanghan_window_cfg(&self) -> Result<Option<ShanghanWindowCfg>, String> {
        match self.get_setting("shanghanWindow")? {
            Some(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    }

    pub fn set_shanghan_window_cfg(&self, cfg: &ShanghanWindowCfg) -> Result<(), String> {
        let json = serde_json::to_string(cfg).map_err(|e| e.to_string())?;
        self.set_setting("shanghanWindow", &json)
    }

    // ---------- 搜索文件索引 ----------

    /// 事务内重建某作用域（'common' = 常用目录 / 'full' = 全盘）的索引：
    /// 先清空该作用域，再批量插入扫描结果。
    /// `rows` 元组：(path, name, is_dir(0/1), ext, size, mtime)
    pub fn rebuild_file_index(
        &self,
        scope: &str,
        rows: &[(String, String, i64, String, i64, i64)],
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM files_fts WHERE scope = ?1", params![scope])
            .map_err(|e| e.to_string())?;
        let mut stmt = tx
            .prepare(
                "INSERT INTO files_fts (name, path, dir, ext, size, mtime, scope) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .map_err(|e| e.to_string())?;
        for (path, name, dir, ext, size, mtime) in rows {
            stmt.execute(params![name, path, dir, ext, size, mtime, scope])
                .map_err(|e| e.to_string())?;
        }
        drop(stmt);
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 增量更新：单条文件 upsert（用于 USN / 目录变更监听的实时写回）。
    pub fn upsert_file(
        &self,
        name: &str,
        path: &str,
        is_dir: bool,
        ext: &str,
        size: i64,
        mtime: i64,
        scope: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM files_fts WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO files_fts (name, path, dir, ext, size, mtime, scope) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![name, path, if is_dir { 1 } else { 0 }, ext, size, mtime, scope],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 增量更新：按路径删除（文件被删除 / 移出索引范围）。
    pub fn remove_file(&self, path: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM files_fts WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 增量更新：一条文件写回多个作用域（如同时属于 common 与盘符 scope）。
    /// 先按 path 清掉所有旧行（避免跨 scope 残留），再逐个 scope 插入。
    pub fn upsert_file_in_scopes(
        &self,
        name: &str,
        path: &str,
        is_dir: bool,
        ext: &str,
        size: i64,
        mtime: i64,
        scopes: &[String],
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM files_fts WHERE path = ?1", params![path])
            .map_err(|e| e.to_string())?;
        for s in scopes {
            conn.execute(
                "INSERT INTO files_fts (name, path, dir, ext, size, mtime, scope) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![name, path, if is_dir { 1 } else { 0 }, ext, size, mtime, s],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// 增量更新：删除某路径及其下所有条目（目录被删除 / 移出时清子树）。
    pub fn remove_tree(&self, prefix: &str) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let like = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        conn.execute(
            "DELETE FROM files_fts WHERE path LIKE ?1 ESCAPE '\\'",
            params![like],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// fts5 trigram 至少要 3 个字符才能切出一个三元组。**短于 3 个字符的查询
    /// （中文 1~2 字词："微信" / "火绒" / "淘宝"）MATCH 恒为 0 条** ——
    /// 索引里根本没有可匹配的 token，必须换一条路（见 `search_files`）。
    ///
    /// 注意必须按「字符数」而非字节数判断：2 个汉字 = 6 字节，用 `len()` 会被
    /// 误判成"够长"，于是又落回 MATCH 路径、又是 0 条。
    const TRIGRAM_MIN_CHARS: usize = 3;

    /// 在索引中按文件名模糊搜索，限定 `scopes`（如 ["common","C"]）。
    /// 返回 (path, name, is_dir, ext, size, mtime)。按修改时间倒序，最新的排最前。
    ///
    /// 两段式：先「前缀匹配」(x%)，不足 `limit` 再用「子串匹配」(%x%) 补足，
    /// 所以以查询开头的文件排在最前。每段的实现按查询长度二选一：
    /// - 字符数 ≥ 3：FTS5 trigram 倒置索引，毫秒级；
    /// - 字符数 < 3：trigram 切不出三元组，改用 `name LIKE` 全表扫描兜底
    ///   （中文 1~2 字词是常态输入，不能没有结果；10 万级索引实测约 40ms）。
    ///
    /// `query` 传**原始**用户输入（未做任何转义），转义在这里统一做：
    /// fts5 短语用 `esc_fts`，LIKE 通配符用 `esc_like`。
    pub fn search_files(
        &self,
        scopes: &[String],
        query: &str,
        limit: i32,
        exts: Option<&[String]>,
        exclude_exts: Option<&[String]>,
        exclude_prefixes: Option<&[String]>,
        sort: &str,
    ) -> Result<Vec<(String, String, i64, String, i64, i64)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let scopes: Vec<String> = if scopes.is_empty() {
            vec!["common".to_string()]
        } else {
            scopes.to_vec()
        };
        let ph: String = (1..=scopes.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        // 排序方式来自设置页「搜索结果排序」；未知值回退按修改时间倒序（最新的排最前）
        let order = match sort {
            "name" => "(CASE WHEN dir = 1 THEN 0 ELSE 1 END), length(name) ASC",
            "size" => "size DESC",
            _ => "mtime DESC",
        };

        // 空列表 = 该大类未勾选任何后缀，直接无结果；None = 不按类型过滤
        if let Some(e) = exts {
            if e.is_empty() {
                return Ok(Vec::new());
            }
        }

        // 匹配条件二选一（前缀段 / 子串段共用同一个条件，只换 pattern）：
        // - 字符数 ≥ 3：fts5 trigram 短语，前缀 `name:"x"*`、子串 `name:"x"`；
        // - 字符数 < 3：LIKE 全表扫描（trigram 索引在这种情况下恒不命中）
        let sp = scopes.len() + 1;
        let (prefix_pat, any_pat, cond) = if query.chars().count() < Self::TRIGRAM_MIN_CHARS {
            let like = esc_like(query);
            (
                format!("{like}%"),
                format!("%{like}%"),
                format!("name LIKE ?{sp} ESCAPE '\\'"),
            )
        } else {
            let esc = esc_fts(query);
            (
                format!("name:\"{esc}\"*"),
                format!("name:\"{esc}\""),
                format!("files_fts MATCH ?{sp}"),
            )
        };

        // 第一段：前缀匹配
        let mut out = Self::search_slice(
            &conn,
            &scopes,
            &ph,
            &cond,
            &prefix_pat,
            exts,
            exclude_exts,
            exclude_prefixes,
            limit,
            order,
        )?;
        // 第二段：前缀不足时再补「子串匹配」（应用层按 path 去重，避免重复）
        if (out.len() as i32) < limit {
            let need = limit - out.len() as i32;
            let taken: std::collections::HashSet<String> =
                out.iter().map(|r| r.0.clone()).collect();
            let mut more = Self::search_slice(
                &conn,
                &scopes,
                &ph,
                &cond,
                &any_pat,
                exts,
                exclude_exts,
                exclude_prefixes,
                need,
                order,
            )?;
            for r in more.drain(..) {
                if !taken.contains(&r.0) {
                    out.push(r);
                }
                if (out.len() as i32) >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// 单段查询：`scope IN (...) AND <cond> {类型/排除过滤}` 取一批结果。
    ///
    /// `cond` 是**已含占位符**的匹配条件（`files_fts MATCH ?N` 或 `name LIKE ?N ESCAPE '\'`），
    /// `pat` 是绑给它的值。占位符编号约定：`?1..?n` = scopes，`?n+1` = pat，
    /// 后面的类型/排除过滤依次递增。
    fn search_slice(
        conn: &rusqlite::Connection,
        scopes: &[String],
        ph: &str,
        cond: &str,
        pat: &str,
        exts: Option<&[String]>,
        exclude_exts: Option<&[String]>,
        exclude_prefixes: Option<&[String]>,
        limit: i32,
        order: &str,
    ) -> Result<Vec<(String, String, i64, String, i64, i64)>, String> {
        // 类型过滤：ext IN (...)（ext 是 UNINDEXED 列，仅作后置过滤）
        let ext_count = match exts {
            Some(e) if !e.is_empty() => e.len(),
            _ => 0,
        };
        // 类型排除：ext NOT IN (...)。ext = ''（文件夹/无后缀）不在排除列表里，天然不受影响
        let ex_count = match exclude_exts {
            Some(e) if !e.is_empty() => e.len(),
            _ => 0,
        };
        // 路径前缀排除（如 "C:%"）：排除盘符时，连「常用」等混合 scope 里
        // 落在该盘的文件也一并排除（scope IN 只是粗筛，前缀过滤才是精准排除）
        let pfx = exclude_prefixes.unwrap_or(&[]);
        let pfx_count = pfx.len();
        let ext_start = scopes.len() + 2;
        let ex_start = ext_start + ext_count;
        let pfx_start = ex_start + ex_count;
        let sli = pfx_start + pfx_count;
        let ext_ph = if ext_count > 0 {
            let parts: Vec<String> = (0..ext_count)
                .map(|i| format!("?{}", ext_start + i))
                .collect();
            format!(" AND ext IN ({})", parts.join(", "))
        } else {
            String::new()
        };
        let ex_ph = if ex_count > 0 {
            let parts: Vec<String> = (0..ex_count)
                .map(|i| format!("?{}", ex_start + i))
                .collect();
            format!(" AND ext NOT IN ({})", parts.join(", "))
        } else {
            String::new()
        };
        let pfx_ph = if pfx_count > 0 {
            (0..pfx_count)
                .map(|i| format!(" AND path NOT LIKE ?{}", pfx_start + i))
                .collect()
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT path, name, dir, ext, size, mtime FROM files_fts \
             WHERE scope IN ({ph}) AND {cond}{ext_ph}{ex_ph}{pfx_ph} \
             GROUP BY path ORDER BY {order} LIMIT ?{sli}"
        );
        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::new();
        for s in scopes {
            params.push(s);
        }
        params.push(&pat);
        if ext_count > 0 {
            if let Some(e) = exts {
                for x in e {
                    params.push(x);
                }
            }
        }
        if ex_count > 0 {
            if let Some(e) = exclude_exts {
                for x in e {
                    params.push(x);
                }
            }
        }
        if pfx_count > 0 {
            for p in pfx {
                params.push(p);
            }
        }
        params.push(&limit);
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(&params[..], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }
}

/// 转义 fts5 短语中的双引号（fts5 短语用双引号包围，内部双引号写成两个双引号）。
/// 顺带去掉换行：文件名里几乎不会有，但用户从别处粘贴多行文本时会带进来。
fn esc_fts(s: &str) -> String {
    s.replace(['\n', '\r'], "").replace('"', "\"\"")
}

/// 转义 LIKE 模式里的通配符（配合 `ESCAPE '\'` 使用）。
/// 不转义的话，用户搜 `%` 或 `_` 会命中一切 —— 得当成字面量查。
fn esc_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for ch in s.chars() {
        match ch {
            '\n' | '\r' => {}
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_trigram_search() {
        let dir = std::env::temp_dir().join("qidomin_fts_test.db");
        let _ = std::fs::remove_file(&dir);
        let db = Db::open(&dir).expect("open db");
        db.rebuild_file_index(
            "common",
            &[
                (
                    "C:\\Users\\me\\WeChat.exe".into(),
                    "WeChat.exe".into(),
                    0,
                    "exe".into(),
                    123,
                    0,
                ),
                (
                    "C:\\Users\\me\\report2023.pdf".into(),
                    "report2023.pdf".into(),
                    0,
                    "pdf".into(),
                    1,
                    0,
                ),
                (
                    "C:\\Users\\me\\Photos".into(),
                    "Photos".into(),
                    1,
                    "".into(),
                    0,
                    0,
                ),
            ],
        )
        .expect("rebuild");
        // 前缀匹配
        let r = db
            .search_files(&["common".into()], "wechat", 50, None, None, None, "name")
            .expect("search");
        assert!(r.iter().any(|x| x.1 == "WeChat.exe"), "prefix wechat");
        // 子串匹配（trigram）
        let r2 = db
            .search_files(&["common".into()], "2023", 50, None, None, None, "name")
            .expect("search");
        assert!(r2.iter().any(|x| x.1 == "report2023.pdf"), "substring 2023");
        // 大小写不敏感（case_sensitive 0）
        let r3 = db
            .search_files(&["common".into()], "WECHAT", 50, None, None, None, "name")
            .expect("search");
        assert!(r3.iter().any(|x| x.1 == "WeChat.exe"), "case insensitive");
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn fts5_ext_filter() {
        let dir = std::env::temp_dir().join("qidomin_fts_ext_test.db");
        let _ = std::fs::remove_file(&dir);
        let db = Db::open(&dir).expect("open db");
        db.rebuild_file_index(
            "common",
            &[
                (
                    "C:\\Users\\me\\WeChat.exe".into(),
                    "WeChat.exe".into(),
                    0,
                    "exe".into(),
                    123,
                    0,
                ),
                (
                    "C:\\Users\\me\\report2023.pdf".into(),
                    "report2023.pdf".into(),
                    0,
                    "pdf".into(),
                    1,
                    0,
                ),
            ],
        )
        .expect("rebuild");
        // 只过滤 exe：应只命中 WeChat.exe
        let r = db
            .search_files(
                &["common".into()],
                "wechat",
                50,
                Some(&["exe".to_string()]),
                None,
                None,
                "name",
            )
            .expect("search");
        assert_eq!(r.len(), 1, "only exe");
        assert!(r[0].1 == "WeChat.exe", "exe match");
        // 只过滤 pdf：应只命中 report2023.pdf
        let r2 = db
            .search_files(
                &["common".into()],
                "2023",
                50,
                Some(&["pdf".to_string()]),
                None,
                None,
                "name",
            )
            .expect("search");
        assert!(r2.iter().any(|x| x.1 == "report2023.pdf"), "pdf match");
        // 空列表 = 无结果（该大类未勾选任何后缀）
        let r3 = db
            .search_files(
                &["common".into()],
                "wechat",
                50,
                Some(&[]),
                None,
                None,
                "name",
            )
            .expect("search");
        assert!(r3.is_empty(), "empty exts => no result");
        // 排除 exe：wechat 应无结果
        let r4 = db
            .search_files(
                &["common".into()],
                "wechat",
                50,
                None,
                Some(&["exe".to_string()]),
                None,
                "name",
            )
            .expect("search");
        assert!(r4.is_empty(), "exclude exe => no wechat");
        let _ = std::fs::remove_file(&dir);
    }

    /// 短查询兜底：中文 1~2 字词（"微信"/"微"）在 trigram 索引里切不出三元组，
    /// MATCH 恒为 0 条 —— 没有 LIKE 兜底的话中文搜索整体失效。
    #[test]
    fn short_query_like_fallback() {
        let dir = std::env::temp_dir().join("qidomin_fts_cjk_test.db");
        let _ = std::fs::remove_file(&dir);
        let db = Db::open(&dir).expect("open db");
        db.rebuild_file_index(
            "common",
            &[
                (
                    "C:\\Users\\me\\桌面\\微信.lnk".into(),
                    "微信.lnk".into(),
                    0,
                    "lnk".into(),
                    1,
                    0,
                ),
                (
                    "C:\\ProgramData\\卸载微信.lnk".into(),
                    "卸载微信.lnk".into(),
                    0,
                    "lnk".into(),
                    1,
                    0,
                ),
                (
                    "C:\\Users\\me\\桌面\\回收站.lnk".into(),
                    "回收站.lnk".into(),
                    0,
                    "lnk".into(),
                    1,
                    0,
                ),
            ],
        )
        .expect("rebuild");

        // 2 字中文：前缀命中（微信.lnk）与子串命中（卸载微信.lnk）都要有
        let sc = vec!["common".to_string()];
        let r = db.search_files(&sc, "微信", 50, None, None, None, "name").expect("search");
        assert!(r.iter().any(|x| x.1 == "微信.lnk"), "2字中文前缀：{r:?}");
        assert!(r.iter().any(|x| x.1 == "卸载微信.lnk"), "2字中文子串：{r:?}");
        // 前缀结果排在子串结果前面
        assert_eq!(r[0].1, "微信.lnk", "前缀优先：{r:?}");

        // 1 个字也要能搜
        let r = db.search_files(&sc, "微", 50, None, None, None, "name").expect("search");
        assert_eq!(r.len(), 2, "1字中文：{r:?}");

        // 3 字中文仍走 trigram（≥3 字符，MATCH 可用；此处验证两条路结果一致）
        let r = db.search_files(&sc, "卸载微", 50, None, None, None, "name").expect("search");
        assert!(r.iter().any(|x| x.1 == "卸载微信.lnk"), "3字中文：{r:?}");

        // 短查询 + 类型过滤：lnk 命中、exe 无结果
        let r = db
            .search_files(&sc, "微信", 50, Some(&["exe".to_string()]), None, None, "name")
            .expect("search");
        assert!(r.is_empty(), "短查询类型过滤应生效：{r:?}");

        // LIKE 通配符必须当字面量：搜 % / _ 不该命中一切
        let r = db.search_files(&sc, "%", 50, None, None, None, "name").expect("search");
        assert!(r.is_empty(), "% 应被转义：{r:?}");
        let r = db.search_files(&sc, "_", 50, None, None, None, "name").expect("search");
        assert!(r.is_empty(), "_ 应被转义：{r:?}");

        let _ = std::fs::remove_file(&dir);
    }

    /// 转义函数本身（含新行剔除）
    #[test]
    fn escape_helpers() {
        assert_eq!(esc_fts("a\"b\nc"), "a\"\"bc");
        assert_eq!(esc_like("50%_\\x"), "50\\%\\_\\\\x");
        // 中文与普通字符原样保留
        assert_eq!(esc_like("微信"), "微信");
    }
}
