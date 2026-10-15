import { invoke } from "@tauri-apps/api/core";
import type {
  AbsorbCfg,
  AppSettings,
  BoxInfo,
  BoxWindowCfg,
  DesktopFile,
  HuangdiWindowCfg,
  HuangliWindowCfg,
  IconItem,
  Note,
  NoteTab,
  NoteWindowCfg,
  OrganizeCommitItem,
  OrganizeCommitResult,
  OrganizePlan,
  OrganizeResult,
  RuleInfo,
  ShanghanWindowCfg,
  StockItem,
  StockQuote,
  StockWindowCfg,
  PoetryWindowCfg,
  SearchItem,
} from "./types";

export function getDesktopFiles(): Promise<DesktopFile[]> {
  return invoke("get_desktop_files");
}

export function getBoxes(): Promise<BoxInfo[]> {
  return invoke("get_boxes");
}

export function getBoxById(id: number): Promise<BoxInfo> {
  return invoke("get_box_by_id", { id });
}

export function getBoxIcons(boxId: number): Promise<IconItem[]> {
  return invoke("get_box_icons", { boxId });
}

/**
 * 新建分区，返回**最终用的分区名**。
 *
 * `name` 传空字符串即可：名字由后端定（按类型取下一个空号，跳过库里和桌面上
 * 已占用的名字）。前端不要自己拼名字 —— 以前用"同类型分区数 + 1"拼，
 * 撞上已有分区名就会报错，新建分区直接失效。
 */
export function createBox(
  name: string,
  color: string,
  boxType?: "software" | "file" | "folder",
  linkPath?: string
): Promise<string> {
  return invoke("create_box", { name, color, boxType, linkPath });
}

export function setBoxLink(id: number, path: string): Promise<void> {
  return invoke("set_box_link", { id, path });
}

export function setBoxSort(
  id: number,
  sortMode: string,
  sortDesc = false
): Promise<void> {
  return invoke("set_box_sort", { id, sortMode, desc: sortDesc });
}

// ---------- 分区：吸附同类文件 ----------

export function getAbsorbCfg(boxId: number): Promise<AbsorbCfg> {
  return invoke("get_absorb_cfg", { boxId });
}

/** 保存吸附配置并立即整理一次，返回本次收进来的文件数 */
export function setAbsorbCfg(
  boxId: number,
  exts: string[],
  auto: boolean
): Promise<OrganizeResult> {
  return invoke("set_absorb_cfg", { boxId, exts, auto });
}

/** 按当前配置再吸附一次 */
export function absorbBoxNow(boxId: number): Promise<OrganizeResult> {
  return invoke("absorb_box_now", { boxId });
}

export function exportLayout(path: string): Promise<void> {
  return invoke("export_layout", { path });
}

export function importLayout(path: string): Promise<void> {
  return invoke("import_layout", { path });
}

export function deleteBox(id: number, restoreFiles: boolean): Promise<void> {
  return invoke("delete_box", { id, restoreFiles });
}

export function renameBox(id: number, name: string): Promise<void> {
  return invoke("rename_box", { id, name });
}

export function mergeBoxes(srcId: number, dstId: number): Promise<void> {
  return invoke("merge_boxes", { srcId, dstId });
}

export function organizeDesktop(): Promise<OrganizeResult> {
  return invoke("organize_desktop");
}

/**
 * 自动整理预览：只算"谁会被搬到哪"，不动任何文件。
 * 返回 items（可执行清单）和 busy（正在写入、本轮跳过的文件）。
 * 用户在预览对话框里确认（可取消勾选）后再调 organizeDesktopCommit。
 */
export function organizeDesktopPlan(): Promise<OrganizePlan> {
  return invoke("organize_desktop_plan");
}

/** 执行用户确认过的整理清单，返回批次 id 供撤销 */
export function organizeDesktopCommit(items: OrganizeCommitItem[]): Promise<OrganizeCommitResult> {
  return invoke("organize_desktop_commit", { items });
}

/** 撤销最近一次整理（按批次记录原路搬回），返回 [搬回数, 批次总数] */
export function undoLastOrganize(): Promise<[number, number]> {
  return invoke("undo_last_organize");
}

/** 最近一次整理批次的信息（batchId、文件数、时间），没有则为 null */
export function getLastOrganizeBatch(): Promise<[string, number, string] | null> {
  return invoke("get_last_organize_batch");
}

export function organizeBox(boxId: number): Promise<OrganizeResult> {
  return invoke("organize_box", { boxId });
}

export function getRules(): Promise<RuleInfo[]> {
  return invoke("get_rules");
}

export function saveRule(rule: RuleInfo): Promise<void> {
  return invoke("save_rule", { rule });
}

export function deleteRule(id: number): Promise<void> {
  return invoke("delete_rule", { id });
}

export function getSettings(): Promise<AppSettings> {
  return invoke("get_settings");
}

export function saveSettings(settings: AppSettings): Promise<void> {
  return invoke("save_settings", { settings });
}

export function getBoxFiles(boxName: string): Promise<DesktopFile[]> {
  return invoke("get_box_files", { boxName });
}

// ---------- 分区窗口 ----------

export function getBoxWindowCfg(id: number): Promise<BoxWindowCfg | null> {
  return invoke("get_box_window_cfg", { id });
}

export function saveBoxWindowCfg(cfg: BoxWindowCfg): Promise<void> {
  return invoke("save_box_window_cfg", { cfg });
}

export function setBoxVisible(id: number, visible: boolean): Promise<void> {
  return invoke("set_box_visible", { id, visible });
}

/**
 * 收起 / 展开分区（胶囊模式）。
 * 收起时后端记住展开几何并缩成胶囊；展开时按记录恢复。
 * 悬停临时展开不走这里（前端直接改窗口尺寸，配置保持"已收起"）。
 */
export function setBoxCollapsed(id: number, collapsed: boolean): Promise<void> {
  return invoke("set_box_collapsed", { id, collapsed });
}

/** 托盘右键菜单动作（toggle/add/organize/settings/quit），见 TrayMenuWindow.tsx */
export function trayMenuAction(action: string): Promise<void> {
  return invoke("tray_menu_action", { action });
}

/** 托盘菜单前端心跳：把 WebView 存活状态打到后端日志 */
export function trayMenuPing(reason: string): Promise<void> {
  return invoke("tray_menu_ping", { reason });
}

/** 收起托盘菜单（后端把窗口挪到屏幕外；窗口常驻，WebView 不挂起） */
export function trayMenuDismiss(): Promise<void> {
  return invoke("tray_menu_dismiss");
}

/** 切换开机自启（写注册表 Run 键） */
export function setAutoStartEnabled(enable: boolean): Promise<void> {
  return invoke("set_auto_start_enabled", { enable });
}

export function startBoxDrag(boxId: number, forceMoved: boolean): Promise<void> {
  return invoke("start_box_drag", { boxId, forceMoved });
}

/** 拉伸吸附候选：其它分区 / 便签 / 屏幕工作区（物理像素）+ 吸附阈值 */
export interface BoxSnapTargets {
  rects: [number, number, number, number][];
  threshold: number;
}

export function getBoxSnapTargets(boxId: number): Promise<BoxSnapTargets> {
  return invoke("get_box_snap_targets", { owner: `box-${boxId}` });
}

export function installShellMenu(): Promise<void> {
  return invoke("install_shell_menu");
}

export function uninstallShellMenu(): Promise<void> {
  return invoke("uninstall_shell_menu");
}

export function isShellMenuInstalled(): Promise<boolean> {
  return invoke("is_shell_menu_installed");
}

// ---------- 快捷方式小箭头 ----------

export function getShortcutArrowHidden(): Promise<boolean> {
  return invoke("get_shortcut_arrow_hidden");
}

/**
 * 隐藏 / 恢复快捷方式小箭头。写入系统级（HKLM）注册表，需要管理员权限
 * （进程已提权则无弹窗，否则弹一次 UAC）。改完就地重建图标缓存刷新，不重启资源管理器。
 */
export function setShortcutArrowHidden(hide: boolean): Promise<void> {
  return invoke("set_shortcut_arrow_hidden", { hide });
}

/**
 * 强制生效：清图标缓存并重启资源管理器（桌面会闪一下），返回结果说明文字。
 * 主要用于个别位置仍是旧箭头、或上次 UAC 被取消后重试。
 */
export function applyShortcutArrowForced(hide: boolean): Promise<string> {
  return invoke("apply_shortcut_arrow_forced", { hide });
}

// ---------- 原生 Shell：打开文件 / 系统右键菜单 ----------

/**
 * 用系统默认程序打开文件 / 文件夹（等价于双击）。
 * 走后端 ShellExecute，不经过 opener 插件，因此不会因权限声明缺失而被静默拒绝。
 */
export function openFile(path: string): Promise<void> {
  return invoke("open_path", { path });
}

/**
 * 在光标处弹出该文件的系统右键菜单（和桌面 / 资源管理器里右键完全一致）。
 * 返回 `"rename"` 表示用户点了菜单里的「重命名」（需要就地改名），否则为 null。
 */
export function showFileMenu(path: string): Promise<string | null> {
  return invoke("show_file_menu", { path });
}

/**
 * 在光标处弹出分区空白处的系统右键菜单（和资源管理器里打开文件夹后空白处右键一致：
 * 粘贴 / 新建 / 查看 / 排序 / 刷新 / 属性 …）。
 */
export function showBoxBackgroundMenu(boxId: number): Promise<void> {
  return invoke("show_box_background_menu", { boxId });
}

/** 剪贴板内容分类：`text` 有文字 / `files` 只有文件 / `empty` 什么都没有 */
export interface ClipboardInfo {
  kind: "text" | "files" | "empty";
  text: string;
  files: string[];
}

/**
 * 读取系统剪贴板（搜索面板「粘贴」按钮用）。
 * 只做读取，不搬任何文件；非文字内容由前端自己提示用户。
 */
export function readClipboard(): Promise<ClipboardInfo> {
  return invoke("read_clipboard");
}

/** 按文件名模糊搜索本地索引；scopes 限定搜索范围（如 ["common","C"]），exts 限定文件类型大类（后缀列表，空=不过滤） */
/** 进入「按一下鼠标识别信号」模式：回传事件 mouse-captured / mouse-capture-cancel */
export function startMouseCapture(): Promise<void> {
  return invoke("start_mouse_capture");
}

export function searchFiles(
  query: string,
  limit?: number,
  scopes?: string[],
  exts?: string[],
  excludeExts?: string[],
  excludeScopes?: string[]
): Promise<SearchItem[]> {
  return invoke("search_files", {
    query,
    limit,
    scopes,
    exts,
    excludeExts,
    excludeScopes,
  });
}

/** 重建搜索索引：drive 指定单盘（如 "C"）只扫该盘；否则按设置重建 */
export function rebuildSearchIndex(
  full?: boolean,
  drive?: string
): Promise<[number, number]> {
  return invoke("rebuild_search_index", { full, drive });
}

/** 清空某个作用域（盘符）的索引，用于取消勾选 */
export function clearIndex(scope: string): Promise<void> {
  return invoke("clear_index", { scope });
}

/** 列出所有可读盘符（大写，如 ["C","D","E"]） */
export function listDrives(): Promise<string[]> {
  return invoke("list_drives");
}

/** 就地重命名（同一目录内），只接受纯文件名；返回新路径。 */
export function renameFile(path: string, newName: string): Promise<string> {
  return invoke("rename_file", { path, newName });
}

/**
 * 用系统原生拖拽拖出一个文件 / 文件夹：可以落到桌面、资源管理器窗口、其它分区，
 * 甚至别的程序里。该 Promise 在用户**松开鼠标（拖拽结束）后**才 resolve。
 *
 * `boxId` 是发起拖拽的分区：拖拽结束后后端靠它排除"落回自己分区"的情况，
 * 万一落点事件没送到目标分区，也能按光标位置兜底把文件搬过去。
 */
export function startFileDrag(path: string, boxId: number): Promise<void> {
  return invoke("start_file_drag", { path, boxId });
}

/** 搜索结果拖出：不关联分区（boxId = null），拖到分区仍会"整理进分区" */
export function startSearchFileDrag(path: string): Promise<void> {
  return invoke("start_file_drag", { path, boxId: null });
}

// ---------- 分区弹出菜单（独立置顶窗口） ----------

/**
 * 打开 / 切换分区菜单。菜单是独立置顶小窗口：分区再小也裁不到它，
 * 也不会被"沉到桌面层"的分区挡住。
 */
export function openBoxMenu(boxId: number): Promise<void> {
  return invoke("open_box_menu", { boxId });
}

/** 菜单量好自身高度后调用：摆正位置 + 定尺寸 + 显示。 */
export function fitBoxMenu(boxId: number, height: number): Promise<void> {
  return invoke("fit_box_menu", { boxId, height });
}

/** 收起分区菜单。 */
export function closeBoxMenu(boxId: number): Promise<void> {
  return invoke("close_box_menu", { boxId });
}

/**
 * 把菜单里点选的操作转交给对应分区窗口执行，然后收起菜单。
 * `close = false` 用于"改完配置让分区刷新一下、菜单先不收起"这类动作。
 */
export function boxMenuAction(boxId: number, action: unknown, close = true): Promise<void> {
  return invoke("box_menu_action", { boxId, action, close });
}

// ---------- 组件：便签 ----------

/** 某一页（标签）下的便签 */
export function getNotes(tabId: number): Promise<Note[]> {
  return invoke("get_notes", { tabId });
}

/** 在指定页新增一条空便签 */
export function addNote(tabId: number): Promise<number> {
  return invoke("add_note", { tabId });
}

// ---- 便签页（标题栏标签栏）----

export function listNoteTabs(): Promise<NoteTab[]> {
  return invoke("list_note_tabs");
}

/** 新建一页，返回其 id */
export function addNoteTab(name: string): Promise<number> {
  return invoke("add_note_tab", { name });
}

export function renameNoteTab(id: number, name: string): Promise<void> {
  return invoke("rename_note_tab", { id, name });
}

/** 删除一页（连同它下面的便签）；后端保证至少留一页 */
export function deleteNoteTab(id: number): Promise<void> {
  return invoke("delete_note_tab", { id });
}

export function saveNote(note: Note): Promise<void> {
  return invoke("save_note", { note });
}

export function deleteNote(id: number): Promise<void> {
  return invoke("delete_note", { id });
}

export function getNotesEnabled(): Promise<boolean> {
  return invoke("get_notes_enabled");
}

/** 勾选 = 显示在桌面，取消 = 隐藏 */
export function setNotesEnabled(enabled: boolean): Promise<void> {
  return invoke("set_notes_enabled", { enabled });
}

export function getNoteWindowCfg(): Promise<NoteWindowCfg> {
  return invoke("get_note_window_cfg");
}

export function saveNoteWindowCfg(cfg: NoteWindowCfg): Promise<void> {
  return invoke("save_note_window_cfg", { cfg });
}

export function startNoteDrag(forceMoved: boolean): Promise<void> {
  return invoke("start_note_drag", { forceMoved });
}

// ---------- 组件：股票 ----------

export function getStocks(): Promise<StockItem[]> {
  return invoke("get_stocks");
}

/** 添加自选股；代码支持 600000 / sh600000 / 00700 / AAPL 等写法 */
export function addStock(code: string): Promise<StockItem> {
  return invoke("add_stock", { code });
}

export function deleteStock(id: number): Promise<void> {
  return invoke("delete_stock", { id });
}

export function reorderStocks(ids: number[]): Promise<void> {
  return invoke("reorder_stocks", { ids });
}

/** 批量拉取实时行情 */
export function fetchStocks(secids: string[]): Promise<StockQuote[]> {
  return invoke("fetch_stocks", { secids });
}

export function getStocksEnabled(): Promise<boolean> {
  return invoke("get_stocks_enabled");
}

export function setStocksEnabled(enabled: boolean): Promise<void> {
  return invoke("set_stocks_enabled", { enabled });
}

export function getStockWindowCfg(): Promise<StockWindowCfg> {
  return invoke("get_stock_window_cfg");
}

export function saveStockWindowCfg(cfg: StockWindowCfg): Promise<void> {
  return invoke("save_stock_window_cfg", { cfg });
}

export function startStockDrag(forceMoved: boolean): Promise<void> {
  return invoke("start_stock_drag", { forceMoved });
}

// ---------- 组件：黄历 ----------
// 历法全部在前端本地推算（lunar-javascript），没有网络请求，这里只有窗口相关命令。

export function getHuangliEnabled(): Promise<boolean> {
  return invoke("get_huangli_enabled");
}

export function setHuangliEnabled(enabled: boolean): Promise<void> {
  return invoke("set_huangli_enabled", { enabled });
}

export function getHuangliWindowCfg(): Promise<HuangliWindowCfg> {
  return invoke("get_huangli_window_cfg");
}

export function saveHuangliWindowCfg(cfg: HuangliWindowCfg): Promise<void> {
  return invoke("save_huangli_window_cfg", { cfg });
}

export function startHuangliDrag(forceMoved: boolean): Promise<void> {
  return invoke("start_huangli_drag", { forceMoved });
}

// ---------- 组件：毛泽东诗词 ----------
// 诗词内容与搜索全部在前端本地完成（内置数据），这里只有开关与窗口相关命令。

export function getPoetryEnabled(): Promise<boolean> {
  return invoke("get_poetry_enabled");
}

export function setPoetryEnabled(enabled: boolean): Promise<void> {
  return invoke("set_poetry_enabled", { enabled });
}

export function getPoetryWindowCfg(): Promise<PoetryWindowCfg> {
  return invoke("get_poetry_window_cfg");
}

export function savePoetryWindowCfg(cfg: PoetryWindowCfg): Promise<void> {
  return invoke("save_poetry_window_cfg", { cfg });
}

export function startPoetryDrag(forceMoved: boolean): Promise<void> {
  return invoke("start_poetry_drag", { forceMoved });
}

// ---------- 组件：黄帝内经 ----------
// 经文与搜索全部在前端本地完成（内置 JSON 数据），这里只有开关与窗口相关命令。

export function getHuangdiEnabled(): Promise<boolean> {
  return invoke("get_huangdi_enabled");
}

export function setHuangdiEnabled(enabled: boolean): Promise<void> {
  return invoke("set_huangdi_enabled", { enabled });
}

export function getHuangdiWindowCfg(): Promise<HuangdiWindowCfg> {
  return invoke("get_huangdi_window_cfg");
}

export function saveHuangdiWindowCfg(cfg: HuangdiWindowCfg): Promise<void> {
  return invoke("save_huangdi_window_cfg", { cfg });
}

export function startHuangdiDrag(forceMoved: boolean): Promise<void> {
  return invoke("start_huangdi_drag", { forceMoved });
}

// ---------- 组件：伤寒论 ----------
// 条文与搜索全部在前端本地完成（内置 JSON 数据），这里只有开关与窗口相关命令。

export function getShanghanEnabled(): Promise<boolean> {
  return invoke("get_shanghan_enabled");
}

export function setShanghanEnabled(enabled: boolean): Promise<void> {
  return invoke("set_shanghan_enabled", { enabled });
}

export function getShanghanWindowCfg(): Promise<ShanghanWindowCfg> {
  return invoke("get_shanghan_window_cfg");
}

export function saveShanghanWindowCfg(cfg: ShanghanWindowCfg): Promise<void> {
  return invoke("save_shanghan_window_cfg", { cfg });
}

export function startShanghanDrag(forceMoved: boolean): Promise<void> {
  return invoke("start_shanghan_drag", { forceMoved });
}
