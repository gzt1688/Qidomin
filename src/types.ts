export interface BoxInfo {
  id: number;
  name: string;
  color: string;
  fileCount: number;
  boxType: "software" | "file" | "folder";
  linkPath: string | null;
  sortMode: string;
  /** 排序方向：false = 升序，true = 降序 */
  sortDesc: boolean;
}

export interface DesktopFile {
  name: string;
  path: string;
  isDir: boolean;
  ext: string;
  size: number;
  modifiedAt: string;
}

export type MatchType = "extension" | "keyword" | "size" | "date";

export interface RuleInfo {
  id: number;
  name: string;
  boxId: number;
  boxName: string;
  matchType: MatchType;
  pattern: string;
  priority: number;
  enabled: boolean;
}

export interface OrganizeResult {
  moved: number;
  failed: number;
  details: { boxName: string; count: number }[];
}

/** 一次「把文件拖进某个分区」的结果（分区之间搬运 / 复制） */
export interface TransferResult {
  /** 移动进来的条目数 */
  moved: number;
  /** 复制进来的条目数 */
  copied: number;
  /** 跳过的条目数（拖回自己所在分区 = 调位置） */
  skipped: number;
  /** 失败的条目数 */
  failed: number;
  /** 失败原因，可直接展示 */
  errors: string[];
}

/** `box-transfer-result` 事件载荷 */
export interface BoxTransferResultMsg {
  boxId: number;
  result: TransferResult;
}

export interface AppSettings {
  organizeShortcuts: boolean;
  autoStart: boolean;
  realTimeOrganize: boolean;
  snapEnabled: boolean;
  defaultIconSize: number;
  boxBgColor: string;
  boxBgOpacity: number;
  /** 分区标题字体颜色（CSS 颜色值），默认绿色 */
  boxTitleColor: string;
  /** 显示桌面（Win+D）时是否保持组件/分区可见（挂为桌面子窗口免疫最小化） */
  keepOnShowDesktop: boolean;
  /** 分区里的文件名是否隐藏后缀名（默认显示，与资源管理器"隐藏已知类型后缀"类似但作用于全部文件） */
  hideFileExtensions: boolean;
  /** 搜索索引是否覆盖全盘（默认只索引常用目录） */
  fullDiskIndex: boolean;
  /** 需要额外索引的盘符列表（如 ["C","D"]），为空则只索引常用目录 */
  indexDrives: string[];
  /** 搜索结果排序：mtime（修改时间最新在前，默认）/ name（文件名）/ size（大小） */
  searchSort: string;
  /** 应用内界面缩放（1.0 = 100%），嫌系统字号小可单独放大 */
  uiZoom: number;
  /** 鼠标快捷键总开关（默认关闭：不抢占任何鼠标键） */
  mouseHotkeysEnabled: boolean;
  /** 鼠标快捷键映射表 */
  mouseHotkeys: MouseBinding[];
}

/** 一条鼠标快捷键映射 */
export interface MouseBinding {
  id: number;
  /** middle | middle2 | x1 | x2 */
  button: string;
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  /** 空格作为修饰键（中键+空格这类组合，避免与软件的中键拖动冲突） */
  space: boolean;
  /**
   * 触发方式："press"（默认，按下即触发）| "spaceAfter"（**鼠标键 + 空格**组合，不分先后：
   * 先按住鼠标键再点空格，或先按住空格再点鼠标键，都能触发。鼠标键先按时放行给当前软件，
   * 适合 3D 软件中键旋转视图；空格先按时会被钩子扣住，不会往当前程序里漏空格）
   */
  mode?: string;
  /** 动作 key，见 MOUSE_ACTIONS */
  action: string;
  enabled: boolean;
}

export interface BoxWindowCfg {
  id: number;
  x: number;
  y: number;
  w: number;
  h: number;
  iconSize: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 分区内文件名的文字颜色，只有 #ffffff（白）/ #000000（黑）两态 */
  labelColor: string;
  /** 显示方式：icon = 大图标网格，list = 竖排列表 */
  viewMode: "icon" | "list";
  /**
   * 手工排列的图标顺序（文件路径）。空 = 按分区当前的排序方式显示。
   * 把分区里的图标拖到别的格子上就会写入这里。
   */
  itemOrder?: string[];
  /** 图标网格水平间距（px），缺省 6 */
  gapX?: number;
  /** 图标网格垂直间距（px），缺省 6 */
  gapY?: number;
}

/** 诗词面板窗口位置/外观 */
export interface PoetryWindowCfg {
  x: number;
  y: number;
  w: number;
  h: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 是否自动轮播 */
  autoRotate: boolean;
  /** 自动轮播间隔（秒） */
  rotateSec: number;
}

/** 黄帝内经面板窗口位置/外观 */
export interface HuangdiWindowCfg {
  x: number;
  y: number;
  w: number;
  h: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 是否自动轮播（随机展示经文） */
  autoRotate: boolean;
  /** 自动轮播间隔（秒） */
  rotateSec: number;
}

/** 伤寒论面板窗口位置/外观 */
export interface ShanghanWindowCfg {
  x: number;
  y: number;
  w: number;
  h: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 是否自动轮播（随机展示条文） */
  autoRotate: boolean;
  /** 自动轮播间隔（秒） */
  rotateSec: number;
}

/** 分区「吸附同类文件」配置 */
export interface AbsorbCfg {
  /** 要吸附的扩展名（小写、不含点） */
  exts: string[];
  /** 是否自动吸附新出现的同类文件 */
  auto: boolean;
}

export interface IconItem {
  name: string;
  path: string;
  ext: string;
  isDir: boolean;
  icon: string;
}

/**
 * 分区菜单（独立置顶窗口）里点选的操作。
 * 菜单只负责“表达意图”，真正的业务逻辑仍留在分区窗口里执行。
 */
export type BoxMenuAction =
  | { kind: "create"; boxType: "file" | "folder" }
  | { kind: "newFolder" }
  | { kind: "linkFolder" }
  | { kind: "iconSize"; value: number }
  | { kind: "viewMode"; value: "icon" | "list" }
  | { kind: "bg"; color: string; opacity: number }
  | { kind: "labelColor"; value: string }
  | { kind: "sort"; value: string }
  | { kind: "gap"; gapX: number; gapY: number }
  | { kind: "locked"; value: boolean }
  | { kind: "absorb" }
  | { kind: "organize" }
  | { kind: "delete" };

/**
 * `box-menu-action` 事件的载荷。
 * Tauri 的定向 emit 对 JS 默认的 `Any` 监听器无效，所以必须带上目标分区 id，
 * 由分区窗口自己判断这条操作是不是发给自己的。
 */
export interface BoxMenuActionMsg {
  boxId: number;
  action: BoxMenuAction;
}

// ---------- 组件：便签 ----------

export interface Note {
  id: number;
  content: string;
  /** 提醒时间 "YYYY-MM-DD HH:MM"，null = 不提醒 */
  remindAt: string | null;
  /** true = 有闹钟（到点响铃），false = 无闹钟（静默提示） */
  remindAlarm: boolean;
  /** 是否已提醒过 */
  fired: boolean;
  /** 所属便签页（标题栏标签）id */
  tabId: number;
  /**
   * 最后一次修改时间 "YYYY-MM-DD HH:MM"（空 = 不显示）。
   * 由前端在正文真正变化时写入，显示在正文末尾。
   */
  modifiedAt?: string;
}

/** 便签页 = 标题栏标签栏上的一页 */
export interface NoteTab {
  id: number;
  name: string;
}

export interface NoteWindowCfg {
  x: number;
  y: number;
  w: number;
  h: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 算式自动计算：敲 `6+3=` 自动补成 `6+3=9`（见 `calc.ts`） */
  calc: boolean;
}

// ---------- 组件：股票 ----------

export interface StockItem {
  id: number;
  code: string;
  /** 行情接口标识，如 "1.600000" */
  secid: string;
  name: string;
  sort: number;
}

export interface StockQuote {
  secid: string;
  code: string;
  name: string;
  /** 最新价 */
  price: number | null;
  /** 涨跌额 */
  change: number | null;
  /** 涨跌幅（%） */
  changePct: number | null;
  open: number | null;
  high: number | null;
  low: number | null;
  prevClose: number | null;
}

// ---------- 组件：黄历 ----------

export interface HuangliWindowCfg {
  x: number;
  y: number;
  w: number;
  h: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
}

export interface StockWindowCfg {
  x: number;
  y: number;
  w: number;
  h: number;
  locked: boolean;
  visible: boolean;
  bgColor: string;
  bgOpacity: number;
  /** 自动刷新间隔（秒） */
  interval: number;
}

/** 快速搜索结果条目（纯文本，无图标） */
export interface SearchItem {
  name: string;
  path: string;
  isDir: boolean;
  ext: string;
  size: number;
  /** 修改时间（Unix 秒；0 = 未知） */
  mtime: number;
}
