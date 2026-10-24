import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  closeBoxMenu,
  createBox,
  deleteBox,
  findNestTarget,
  getAbsorbCfg,
  getBoxById,
  getBoxIcons,
  getBoxWindowCfg,
  getSettings,
  hideNestedRadial,
  nestRadialBox,
  openBoxMenu,
  openFile,
  organizeBox,
  renameBox,
  renameFile,
  saveBoxWindowCfg,
  setAbsorbCfg,
  setBoxCollapsed,
  setBoxLink,
  setBoxSort,
  showBoxBackgroundMenu,
  showFileMenu,
  showNestedRadial,
  startBoxDrag,
  startFileDrag,
  unnestRadialBox,
  getBoxSnapTargets,
} from "../api";
import { magnetSnap, resizeSnap } from "../snap";
import { displayFileName, setHideFileExtensions } from "../format";
import { applyBoxTitleColor } from "../theme";
import type {
  BoxInfo,
  BoxMenuActionMsg,
  BoxTransferResultMsg,
  BoxWindowCfg,
  IconItem,
} from "../types";

/** 把 invoke 抛出来的错误变成一句能给用户看的话 */
function errText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/** 胶囊尺寸（逻辑像素）。必须与后端 boxwin.rs 的 CAPSULE_W / CAPSULE_H 一致：
 *  悬停临时展开/收起由前端直接改窗口尺寸，两边数字对不上就会越收越大。 */
const CAPSULE_W = 150;
const CAPSULE_H = 34;

/** 两个图标列表是否完全一致（顺序 + 关键字段）。内容没变就复用旧数组引用。 */
function sameIconItems(a: IconItem[], b: IconItem[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    const y = b[i];
    if (
      x.path !== y.path ||
      x.name !== y.name ||
      x.ext !== y.ext ||
      x.isDir !== y.isDir ||
      x.icon !== y.icon
    ) {
      return false;
    }
  }
  return true;
}

/** 吸附面板的文件类型分组：整组勾选，剩下的零散扩展名走「其他」输入框 */
const EXT_GROUPS: { key: string; label: string; exts: string[] }[] = [
  {
    key: "image",
    label: "图片",
    exts: ["jpg", "jpeg", "png", "gif", "bmp", "webp", "ico", "svg", "tif", "tiff", "heic", "raw"],
  },
  {
    key: "video",
    label: "视频",
    exts: ["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "rmvb", "3gp"],
  },
  {
    key: "audio",
    label: "音频",
    exts: ["mp3", "wav", "flac", "aac", "ogg", "m4a", "wma", "ape", "mid", "amr"],
  },
  {
    key: "doc",
    label: "文档",
    exts: [
      "doc", "docx", "pdf", "txt", "md", "rtf",
      "xls", "xlsx", "csv", "ppt", "pptx", "wps", "et", "dps",
    ],
  },
  {
    key: "archive",
    label: "压缩包",
    exts: ["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "cab"],
  },
  {
    key: "model3d",
    label: "3D 模型",
    exts: [
      "stl", "step", "stp", "obj",
      "fbx", "3mf", "igs", "iges", "dae", "gltf", "glb", "blend",
    ],
  },
  {
    key: "program",
    label: "程序/安装包",
    exts: ["exe", "msi", "apk", "bat", "cmd", "jar", "dmg", "deb"],
  },
  {
    key: "shortcut",
    label: "快捷方式",
    exts: ["lnk", "url"],
  },
  {
    key: "code",
    label: "代码/脚本",
    exts: ["py", "js", "ts", "java", "c", "cpp", "h", "json", "xml", "html", "css", "sh", "sql"],
  },
];

const KNOWN_EXTS = new Set(EXT_GROUPS.flatMap((g) => g.exts));

/** 把用户输入的一串扩展名拆成规范形式（小写、去点、去重） */
function parseExts(text: string): string[] {
  const out: string[] = [];
  for (const raw of text.split(/[,，;；\s]+/)) {
    const e = raw.trim().replace(/^\.+/, "").toLowerCase();
    if (e && !out.includes(e)) out.push(e);
  }
  return out;
}

function hexToRgba(hex: string, alpha: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec((hex || "").trim());
  if (!m) return `rgba(255,255,255,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

/**
 * 应用"手工排列顺序"：有就按它排；不在顺序里的（刚拖进来的新文件）
 * 保持后端给的顺序，排在后面。
 */
function applyItemOrder(list: IconItem[], order?: string[]): IconItem[] {
  if (!order || order.length === 0) return list;
  const pos = new Map(order.map((p, i) => [p, i]));
  const last = Number.MAX_SAFE_INTEGER;
  return [...list].sort((a, b) => (pos.get(a.path) ?? last) - (pos.get(b.path) ?? last));
}

export default function BoxWindow({ boxId }: { boxId: number }) {
  const [box, setBox] = useState<BoxInfo | null>(null);
  const [cfg, setCfg] = useState<BoxWindowCfg>({
    id: boxId,
    x: 0,
    y: 0,
    w: 330,
    h: 250,
    iconSize: 48,
    locked: false,
    visible: true,
    bgColor: "#ffffff",
    bgOpacity: 0,
    labelColor: "#ffffff",
    viewMode: "icon",
  });
  const [items, setItems] = useState<IconItem[]>([]);
  /** 饼型菜单外观（设置页/右键菜单全局配置）：圆钮直径 / 文件最小间距 / 文件尺寸 / 标题 / 图标 */
  const [radialPref, setRadialPref] = useState({
    size: 54,
    ringGap: 28,
    itemSize: 66,
    title: "",
    icon: "",
  });
  /** 最新 items / cfg 的镜像：事件回调里要读当前值，用 ref 免得把监听反复重建 */
  const itemsRef = useRef<IconItem[]>([]);
  const cfgRef = useRef(cfg);
  /** 正在拖的图标 + 拖动前的顺序（没落在本分区时要还原回去） */
  const dragInfo = useRef<{ path: string; snapshot: IconItem[]; dropped: boolean } | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [editing, setEditing] = useState(false);
  const [editName, setEditName] = useState("");
  // 就地改名：右键菜单里选「重命名」后，把该项的名称换成输入框
  const [renaming, setRenaming] = useState<{ path: string; name: string } | null>(null);
  const renamingRef = useRef<{ path: string; name: string } | null>(null);
  const renameInputRef = useRef<HTMLInputElement | null>(null);
  // 关联文件夹标题栏搜索：纯前端按文件名过滤，实时增删也始终基于最新 items
  const [search, setSearch] = useState("");
  const [bg, setBg] = useState({ color: "#ffffff", opacity: 0 });
  /** 拖放搬运结果提示（成功 / 失败原因）。以前全部静默丢弃，用户只看到"没反应" */
  const [toast, setToast] = useState<{ text: string; bad: boolean } | null>(null);
  const toastTimer = useRef<number | null>(null);
  // 吸附同类文件
  const [absorbOpen, setAbsorbOpen] = useState(false);
  const [absorbSelected, setAbsorbSelected] = useState<string[]>([]);
  const [absorbCustom, setAbsorbCustom] = useState("");
  const [absorbAuto, setAbsorbAuto] = useState(false);
  const [absorbBusy, setAbsorbBusy] = useState(false);
  const [absorbMsg, setAbsorbMsg] = useState("");
  const editRef = useRef<HTMLInputElement>(null);

  /**
   * 请求系统键盘焦点。
   * 分区窗口带 WS_EX_NOACTIVATE：点击不会激活窗口（否则每一下点击都会让
   * 全部分区闪一次、打断双击），所以需要键入的输入框（改名 / 搜索）必须
   * 显式申请焦点；已聚焦时跳过，避免无谓的置顶+沉回闪烁。
   */
  const requestKeyboardFocus = useCallback(() => {
    const win = getCurrentWindow();
    win
      .isFocused()
      .then((f) => {
        if (!f) win.setFocus().catch(() => {});
      })
      .catch(() => {});
  }, []);
  /** 上一次点击标题的时间/位置：指针捕获会吞掉原生 dblclick，只能自己判定双击 */
  const titleTap = useRef<{ t: number; x: number; y: number } | null>(null);
  /** 根节点：只为量自身宽度做标题栏自适应（见 headTight） */
  const rootRef = useRef<HTMLDivElement | null>(null);
  /**
   * 标题栏拥挤程度：0 全显示 / 1 收起"项数" / 2 连搜索框也收起。
   *
   * 为什么必须自适应：锁定后分区既不能拖动也不能改尺寸，而解锁按钮就在标题栏右侧。
   * 分区一旦被拉得很窄，右侧按钮会被挤出可视区 —— 用户就再也点不到解锁，彻底卡死。
   * 判据用元素自身宽度（CSS 像素）：阈值和按钮尺寸同单位，因此界面缩放对它没有影响。
   */
  const [headTight, setHeadTight] = useState(0);

  const load = useCallback(async () => {
    try {
      // 四个请求互不依赖，一起发。原来 cfg / icons 是串在后面的，
      // 每次文件变动（拖入、改名、整理）都要多等两个来回，累积起来就是"一顿一顿"。
      const [b, s, c, list] = await Promise.all([
        getBoxById(boxId),
        getSettings(),
        getBoxWindowCfg(boxId),
        getBoxIcons(boxId),
      ]);
      setBox(b);
      applyBoxTitleColor(s.boxTitleColor);
      // 「分区文件隐藏后缀名」开关（displayFileName 读这个模块级标记）
      setHideFileExtensions(s.hideFileExtensions === true);
      setRadialPref({
        size: s.radialBtnSize > 0 ? s.radialBtnSize : 54,
        ringGap: s.radialRingGap >= 10 ? s.radialRingGap : 28,
        itemSize: s.radialItemSize > 0 ? s.radialItemSize : 66,
        title: s.radialTitle || "",
        // 图标每分区独立：分区自己设过就用分区的，没设过回退全局默认
        icon: c?.radialIcon || s.radialIcon || "",
      });
      if (c) {
        setCfg(c);
        setBg({
          color: c.bgColor || s.boxBgColor || "#ffffff",
          opacity: c.bgOpacity ?? s.boxBgOpacity ?? 0,
        });
      } else {
        setBg({ color: s.boxBgColor || "#ffffff", opacity: s.boxBgOpacity || 0 });
      }
      // 后端给的是"按排序方式排"的结果；用户手工调过位置就按手工顺序来
      const ordered = applyItemOrder(list, c?.itemOrder);
      // 内容没变就保留原数组引用：filteredItems 的 useMemo 因此不会重算，
      // 几百个图标条目也就不会被整体重建（文件刷新、改菜单设置都会走这里）。
      setItems((prev) => (sameIconItems(prev, ordered) ? prev : ordered));
    } catch (e) {
      console.error("加载分区失败", e);
    }
  }, [boxId]);

  // 设置变化 → 重载。后端每次保存都广播（饼型图标/尺寸需要实时生效），
  // 拖滑块会产生高频保存 → 前端 120ms trailing 节流，避免分区窗口空转重载。
  const reloadTimer = useRef<number | null>(null);
  useEffect(() => {
    const un = listen<boolean>("settings-changed", () => {
      if (reloadTimer.current != null) return; // 已排程，最后落定那次会执行
      reloadTimer.current = window.setTimeout(() => {
        reloadTimer.current = null;
        load();
      }, 120);
    });
    return () => {
      un.then((f) => f());
      if (reloadTimer.current != null) window.clearTimeout(reloadTimer.current);
    };
  }, [load]);

  // items / cfg 镜像，供事件回调读取当前值
  useEffect(() => {
    itemsRef.current = items;
  }, [items]);

  useEffect(() => {
    cfgRef.current = cfg;
  }, [cfg]);

  /** 搜索过滤后的文件列表（仅关联文件夹分区的标题栏搜索框在使用） */
  const filteredItems = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return items;
    return items.filter(
      (it) => it.name.toLowerCase().includes(q) || it.path.toLowerCase().includes(q)
    );
  }, [items, search]);

  useEffect(() => {
    load();
  }, [load]);

  useEffect(() => {
    const un = listen<number>("box-files-changed", (e) => {
      if (e.payload === boxId) load();
    });
    return () => {
      un.then((f) => f());
    };
  }, [load, boxId]);

  /** 弹出搬运结果提示，几秒后自动消失 */
  const showToast = useCallback((text: string, bad: boolean) => {
    if (toastTimer.current != null) window.clearTimeout(toastTimer.current);
    setToast({ text, bad });
    toastTimer.current = window.setTimeout(() => setToast(null), bad ? 6000 : 2600);
  }, []);

  /** Shell 命令（删除到回收站 / 剪切 / 粘贴…）执行完之后的刷新定时器 */
  const menuTimers = useRef<number[]>([]);

  /** 菜单里的活是 Shell 干的：函数返回时目录**通常**已经变了，但"删除到回收站"
   *  这类是 Shell 异步完成的（还要等回收站那边写完），立刻刷可能刷早了。
   *  所以立刻刷一次，再补两次延迟刷新，把命令刚落下的那几百毫秒盖住。 */
  const refreshAfterShell = useCallback(() => {
    menuTimers.current.forEach((t) => window.clearTimeout(t));
    menuTimers.current = [];
    load();
    for (const delay of [180, 600]) {
      menuTimers.current.push(window.setTimeout(() => load(), delay));
    }
  }, [load]);

  useEffect(
    () => () => {
      if (toastTimer.current != null) window.clearTimeout(toastTimer.current);
      menuTimers.current.forEach((t) => window.clearTimeout(t));
      menuTimers.current = [];
    },
    []
  );

  // 拖放搬运结果：成功报条数，失败报原因（跨盘、文件被占用、权限不足都会走这里）
  useEffect(() => {
    const un = listen<BoxTransferResultMsg>("box-transfer-result", (e) => {
      if (e.payload.boxId !== boxId) return;
      const r = e.payload.result;
      const parts: string[] = [];
      if (r.moved > 0) parts.push(`移动 ${r.moved} 项`);
      if (r.copied > 0) parts.push(`复制 ${r.copied} 项`);
      if (r.failed > 0) {
        parts.push(`${r.failed} 项失败`);
        if (r.errors.length > 0) parts.push(r.errors[0]);
      }
      if (parts.length === 0) return;
      showToast(parts.join("，"), r.failed > 0);
      // 失败时把完整原因也打到控制台，方便排查（列表里只放第一条）
      if (r.failed > 0) console.error("搬运失败", r.errors);
    });
    return () => {
      un.then((f) => f());
    };
  }, [boxId, showToast]);

  // ---------- 胶囊模式 ----------
  // cfg.collapsed = true 时窗口收成小胶囊。三种展开路径：
  //  1. 点击胶囊 → 永久展开（后端 set_box_collapsed(false) 恢复几何、清 collapsed）
  //  2. 悬停胶囊 → 临时展开（前端直接把窗口放大到 expandedGeom，配置仍是"已收起"，
  //     重启后还是胶囊）；鼠标离开窗口 1.2s 后自动缩回
  //  3. 分区菜单「展开」→ 同路径 1
  const [tempExpanded, setTempExpanded] = useState(false);
  const hoverOpenTimer = useRef<number | null>(null);
  const hoverCloseTimer = useRef<number | null>(null);
  /** 胶囊按下位置：pointerup 时区分"点击展开"和"拖动结束"（拖动不展开） */
  const capsuleDown = useRef<{ x: number; y: number } | null>(null);
  /** 尺寸补间动画的 rAF 句柄：新动画开始/后端接管尺寸时要取消旧的，避免打架 */
  const sizeAnimRaf = useRef<number | null>(null);
  /** 动画代际号：cancel/新动画都会自增，旧的异步回调（outerSize().then / rAF）比对序号作废 */
  const sizeAnimSeq = useRef(0);

  const clearHoverTimers = useCallback(() => {
    if (hoverOpenTimer.current != null) {
      window.clearTimeout(hoverOpenTimer.current);
      hoverOpenTimer.current = null;
    }
    if (hoverCloseTimer.current != null) {
      window.clearTimeout(hoverCloseTimer.current);
      hoverCloseTimer.current = null;
    }
  }, []);

  const cancelSizeAnim = useCallback(() => {
    sizeAnimSeq.current += 1;
    if (sizeAnimRaf.current != null) {
      cancelAnimationFrame(sizeAnimRaf.current);
      sizeAnimRaf.current = null;
    }
  }, []);

  useEffect(
    () => () => {
      clearHoverTimers();
      cancelSizeAnim();
    },
    [clearHoverTimers, cancelSizeAnim]
  );

  /**
   * 窗口尺寸补间：w/h 从当前尺寸缓动到目标。
   * anchor="topleft"（默认）：左上角钉死 —— 胶囊展开/收起用；
   * anchor="center"：窗口中心钉死 —— 饼型菜单用，圆钮原地不动、气泡向四周绽放。
   * 逐帧 setSize+setPosition（Windows 上是 SetWindowPos），帧数少、开销可忽略。
   * outerSize()/outerPosition() 是异步的，快速连续触发会让多个动画的 then 交错
   * 返回，所以用代际号：只有最新代际才允许驱动窗口几何。
   */
  const animateSizeTo = useCallback(
    (tw: number, th: number, dur: number, anchor: "topleft" | "center" = "topleft") => {
      cancelSizeAnim();
      const seq = sizeAnimSeq.current;
      const win = getCurrentWindow();
      const t0 = performance.now();
      Promise.all([win.outerSize(), anchor === "center" ? win.outerPosition() : null])
        .then(([s, p0]) => {
          if (seq !== sizeAnimSeq.current) return; // 已被更新的动画/后端接管
          const sw = s.width;
          const sh = s.height;
          const tick = (now: number) => {
            if (seq !== sizeAnimSeq.current) return; // 同上，作废
            const t = Math.min(1, (now - t0) / dur);
            const e = 1 - Math.pow(1 - t, 3); // easeOutCubic
            const w = Math.max(1, Math.round(sw + (tw - sw) * e));
            const h = Math.max(1, Math.round(sh + (th - sh) * e));
            if (p0) {
              // 中心锚定：左上角 = 初始位置 - (当前增量)/2
              const x = p0.x - Math.round((w - sw) / 2);
              const y = p0.y - Math.round((h - sh) / 2);
              win.setPosition(new PhysicalPosition(x, y)).catch(() => {});
            }
            win.setSize(new PhysicalSize(w, h)).catch(() => {});
            if (t < 1) {
              sizeAnimRaf.current = requestAnimationFrame(tick);
            } else {
              sizeAnimRaf.current = null;
            }
          };
          sizeAnimRaf.current = requestAnimationFrame(tick);
        })
        .catch(() => {});
    },
    [cancelSizeAnim]
  );

  /** 悬停临时展开：补间放大到收起前的几何。expandedGeom 缺失（老数据）就不动，点击仍可用。 */
  const doHoverExpand = useCallback(() => {
    const g = cfgRef.current.expandedGeom;
    if (!g) return;
    // 内容先切成完整布局：窗口从小铺开时内容逐渐露出，正是"丝滑"的观感
    setTempExpanded(true);
    animateSizeTo(g[2], g[3], 220);
  }, [animateSizeTo]);

  /** 临时收起：窗口补间缩回胶囊尺寸（配置不动，仍是"已收起"），缩完再切胶囊布局 */
  const doHoverCollapse = useCallback(() => {
    getCurrentWindow()
      .scaleFactor()
      .then((sf) =>
        animateSizeTo(Math.round(CAPSULE_W * sf), Math.round(CAPSULE_H * sf), 120)
      )
      .catch(() => {})
      .finally(() => setTempExpanded(false));
  }, [animateSizeTo]);

  /** 鼠标悬上胶囊 → 延迟 150ms 自动临时展开（设置里关掉"悬停展开"就只响应点击） */
  const scheduleHoverExpand = useCallback(() => {
    clearHoverTimers();
    if (cfgRef.current.capsuleHover === false) return;
    hoverOpenTimer.current = window.setTimeout(() => {
      hoverOpenTimer.current = null;
      doHoverExpand();
    }, 150);
  }, [clearHoverTimers, doHoverExpand]);

  // ---------- 饼型菜单分区（radial） ----------
  // 平时是一个小圆钮（后端按 radial_style + collapsed 建窗），单击弹出一圈气泡。
  // 展开是纯前端行为：窗口临时放大（配置仍是收起态，重启后还是圆钮）。

  const isRadial = box?.boxType === "radial";
  /** 展开前的窗口矩形 [x,y,w,h]（物理像素）：收回时精确还原，不依赖任何换算 */
  const radialSavedRect = useRef<[number, number, number, number] | null>(null);
  /** 展开时的物理/css 像素比（DPI 换算用，展开期间恒定） */
  const radialKRef = useRef(1);
  /** 圆钮中心 css 屏幕坐标：展开期间钉死不动，布局/几何全以它为锚 */
  const radialBtnCss = useRef<[number, number]>([0, 0]);
  /** 收回中：stage 播 CSS 收拢动画，播完才真正把窗口缩回圆钮（避免窗口闪） */
  const [radialClosing, setRadialClosing] = useState(false);
  /** 圆钮悬停（JS 事件驱动）：透明无激活窗口上 CSS :hover 不可靠，
   *  数量角标的显隐用 pointerenter/leave 状态控制 */
  const [btnHover, setBtnHover] = useState(false);
  /** 展开用的工作区矩形（css px，去掉了任务栏）：
   *  WebView2 的 window.screen.avail* 在透明无激活窗口上可能返回整块屏幕，
   *  圆钮贴着任务栏时文件会铺进任务栏区域 —— 以 Tauri 的 Monitor.workArea 为准 */
  const radialWorkArea = useRef<{ x: number; y: number; w: number; h: number } | null>(null);

  /** 饼型环半径（css px）：「文件间距」是相邻文件的最小间距，
   *  半径按周长需求算 —— ring = n×(尺寸+间距) / 2π + 尺寸/2；
   *  再与"不压住中心钮"的保底半径取大者。任何数量都不重叠。 */
  const radialRingRadius = useCallback(
    (n: number) => {
      const it = radialPref.itemSize;
      const gap = Math.max(10, radialPref.ringGap);
      const need = (Math.max(1, n) * (it + gap)) / (2 * Math.PI) + it / 2;
      // 保底：中心钮半径(≈size/2) + 文件半径 + 6px 呼吸
      const floor = radialPref.size / 2 + it / 2 + 6;
      return Math.max(floor, need);
    },
    [radialPref.ringGap, radialPref.itemSize, radialPref.size]
  );

  /** 矩形环绕布局：**按边分组对齐** —— 顶边一排（同一 y）、底边一排（同一 y）、
   *  左边一列（同一 x）、右边一列（同一 x），不是沿周长连续散点（那样跨过角的
   *  文件歪歪扭扭，看不出"矩形"）。
   *  `skip`：被砍掉的边（屏幕外方向）——圆钮贴屏幕边缘时那条边放不下，直接
   *  不排文件，形成凹口朝屏外的 U/L 形围绕；文件按剩余边数均分（顺时针序，
   *  前面几条边各多 1 个）。边内等距分布（单文件居中于边中点）；半宽/半高
   *  分别按顶底、左右边的「间距 ≥ gap」需求反推，任何数量都不重叠、不压圆钮。 */
  const radialRectLayout = useCallback(
    (
      n: number,
      skip?: { top?: boolean; right?: boolean; bottom?: boolean; left?: boolean },
      /** 间距覆盖（边缘兜底时收缩用）；缺省用设置值 */
      gapOverride?: number
    ) => {
      const it = radialPref.itemSize;
      const gap = Math.max(10, gapOverride ?? radialPref.ringGap);
      const count = Math.max(1, n);
      const half = it / 2;
      // 剩余边（保持顺时针序）；全被砍（屏幕比内容还小）退回顶+底两行，
      // 交给外层的平移/半环逻辑兜底
      const edges: ("top" | "right" | "bottom" | "left")[] = [];
      if (!skip?.top) edges.push("top");
      if (!skip?.right) edges.push("right");
      if (!skip?.bottom) edges.push("bottom");
      if (!skip?.left) edges.push("left");
      if (edges.length === 0) edges.push("top", "bottom");
      const m = edges.length;
      const base = Math.floor(count / m);
      const extra = count % m;
      const c: Record<string, number> = { top: 0, right: 0, bottom: 0, left: 0 };
      edges.forEach((e, i) => {
        c[e] = base + (i < extra ? 1 : 0);
      });
      const cnts = [c.top, c.right, c.bottom, c.left];
      // 半宽 a：顶/底行两端文件的中心 x = ±(a-half)。**左右列必须与这两端对齐**
      //（列 x 也取 ±(a-half)），这样顶左角文件+左列连成一条直线列 —— 曾用列 x=±a，
      // 比顶行两端文件外凸半个文件位，角上是断的（用户截图反馈）。
      const needA = (Math.max(cnts[0], cnts[2]) - 1) * (it + gap);
      // 下限 = clear + half：**列文件与圆钮的欧氏净距必须 ≥ clear**（外层 fits
      // 护栏要求所有文件 hypot ≥ rb+half+6，否则整个矩形被丢进"弧形模式"——
      // 表现就是矩形展开变圆形）。列文件最近点 x = a-half，故 a ≥ clear+half。
      // 曾用 rb+half+6 当下限，列文件贴着圆钮水平排时 hypot = a-half < clear，
      // fits 判定失败 → 矩形退化成圆形（用户实测"矩形又变成圆形"）。
      const floorA = radialPref.size / 2 + it + 6;
      const a = Math.max(needA / 2 + half, floorA, it * 1.9 + gap / 2); // 下限保证"矩形感"
      // 半高 b：左右列文件纵向铺在顶/底行**之间**（与两排上下净距 ≥ gap），
      // 列中心活动范围 ±(b-it-gap)；顶/底行（y=±b）与圆钮的净距同样 ≥ clear
      const needB = (Math.max(cnts[1], cnts[3]) - 1) * (it + gap);
      const b = Math.max(
        needB / 2 + it + gap,
        floorA,
        (a * 3) / 4
      );
      // 一条边内均匀分布（相对边中点）：两端各让出半个文件位。
      // **cnt=0 必须返回 []**（`cnt<=1` 会给被砍掉的边塞一个幽灵文件，
      // 平移后离圆钮过近导致 fits 失败 → 整个矩形走弧形兜底"变圆形"）
      const spread = (cnt: number, len: number): number[] => {
        if (cnt <= 0) return [];
        if (cnt === 1) return [0];
        const span = len - it;
        const step = span / (cnt - 1);
        return Array.from({ length: cnt }, (_, i) => -span / 2 + step * i);
      };
      const cx2 = a - half; // 行两端 / 左右列共用的中心坐标（对齐的关键）
      const pts: [number, number][] = [];
      // 顶边：同一行 y=-b，从左到右
      spread(cnts[0], 2 * a).forEach((x) => pts.push([x, -b]));
      // 右边：同一列 x=+cx2（与顶行最右文件对齐），从上到下铺在两排之间
      spread(cnts[1], 2 * b - it - 2 * gap).forEach((y) => pts.push([cx2, y]));
      // 底边：同一行 y=+b，从左到右
      spread(cnts[2], 2 * a).forEach((x) => pts.push([x, b]));
      // 左边：同一列 x=-cx2（与顶行最左文件对齐），从上到下
      spread(cnts[3], 2 * b - it - 2 * gap).forEach((y) => pts.push([-cx2, y]));
      return { w: 2 * a + it + 36, h: 2 * b + it + 36, pts };
    },
    [radialPref.ringGap, radialPref.itemSize, radialPref.size]
  );

  /** 展开布局：一切以「圆钮中心」为原点（css px）。原则：**圆钮位置绝不动**。
   *  屏幕装得下 → 环/矩形以圆钮为心；装不下 → 整体往空余方向最小平移；
   *  平移会让文件贴/压圆钮 → 改朝空余方向扇形（半环）展开。
   *  返回文件中心点数组（相对圆钮）、窗口 css 尺寸、圆钮在窗口内的位置。 */
  const computeRadialLayout = useCallback(
    (style: "pie" | "grid" | "rect" | "line", n: number, names: boolean) => {
      const it = radialPref.itemSize;
      const rb = radialPref.size / 2;
      const gap = Math.max(10, radialPref.ringGap);
      const cnt = Math.max(1, n);
      const clear = rb + it / 2 + 6; // 文件与圆钮的最小净距
      const PAD = 8; // 屏幕边缘呼吸
      const M = 22; // 窗口内容外边距（阴影/绽放余量）
      const ih = it / 2 + (names ? 12 : 0); // 文件纵向半高（名字行向下多占一截）
      // 屏幕可用区（工作区，去掉任务栏）与圆钮中心（css 屏幕坐标，展开时已换算好）
      const bcx = radialBtnCss.current[0];
      const bcy = radialBtnCss.current[1];
      // 优先用 Tauri Monitor.workArea（物理像素 ÷ k 换算回 css px）；
      // WebView2 的 window.screen.avail* 在透明无激活窗口上不可靠（可能报整块屏）
      let sx = (window.screen as Screen & { availLeft: number }).availLeft;
      let sy = (window.screen as Screen & { availTop: number }).availTop;
      let sw = window.screen.availWidth;
      let sh = window.screen.availHeight;
      const wa = radialWorkArea.current;
      if (wa) {
        sx = wa.x;
        sy = wa.y;
        sw = wa.w;
        sh = wa.h;
      }
      // 基础布局：文件中心相对「环心」
      let offs: [number, number][];
      /** 矩形候选能否放进工作区（包围盒塞得下 + 平移后全部文件 ≥ clear） */
      const rectFits = (pts: [number, number][]) => {
        const ih2 = it / 2 + (names ? 12 : 0);
        const xs = pts.map((p) => p[0]);
        const ys = pts.map((p) => p[1]);
        const loX = sx + PAD + it / 2 - bcx - Math.min(...xs);
        const hiX = sx + sw - PAD - it / 2 - bcx - Math.max(...xs);
        const loY = sy + PAD + ih2 - bcy - Math.min(...ys);
        const hiY = sy + sh - PAD - ih2 - bcy - Math.max(...ys);
        if (loX > hiX || loY > hiY) return false;
        const vx = Math.min(hiX, Math.max(loX, 0));
        const vy = Math.min(hiY, Math.max(loY, 0));
        return pts.every(
          ([ox, oy]) => Math.hypot(ox + vx, oy + vy) >= rb + it / 2 + 6
        );
      };
      if (style === "rect") {
        // 边缘凹形适配：某条边的外沿出工作区就砍掉它（凹口朝屏外的 U/L 形），
        // 文件重分到剩余边。**砍到「fits 通过」即停**——继续盲目砍会让单边
        // 塞进全部文件（超长 → fits 失败 → 误走弧形"变圆形"）。
        // fits 仍不过 → 间距收缩重排（60% → 最小 6，矩形形状不丢）；
        // 全失败（小屏+超多文件，模拟中 <5% 极端组合）才落到弧形兜底。
        offs = radialRectLayout(cnt).pts;
        for (const gapTry of [
          Math.max(10, radialPref.ringGap),
          Math.max(10, Math.round(radialPref.ringGap * 0.6)),
          6,
        ]) {
          let skip = { top: false, right: false, bottom: false, left: false };
          let res = radialRectLayout(cnt, undefined, gapTry);
          for (let iter = 0; iter < 4; iter++) {
            if (rectFits(res.pts)) break; // 放得下就停，不再多砍
            const fa = (res.w - it - 36) / 2; // 半宽（行两端中心 ±(a-half) + half）
            const fb = (res.h - it - 36) / 2; // 半高（底行下沿含名字行）
            const s2 = {
              top: bcy - fb - it / 2 < sy + PAD,
              bottom: bcy + fb + (names ? 12 : 0) > sy + sh - PAD,
              left: bcx - fa < sx + PAD,
              right: bcx + fa > sx + sw - PAD,
            };
            if (
              (s2.top === skip.top &&
                s2.right === skip.right &&
                s2.bottom === skip.bottom &&
                s2.left === skip.left) ||
              (s2.top && s2.right && s2.bottom && s2.left)
            ) {
              break; // 稳定 / 四边全放不下：交给下一档间距或弧形兜底
            }
            skip = s2;
            res = radialRectLayout(cnt, skip, gapTry);
          }
          if (rectFits(res.pts)) {
            offs = res.pts;
            break;
          }
          if (gapTry === 6) offs = res.pts; // 全失败：至少用最紧一档（弧形兜底）
        }
      } else if (style === "line") {
        const half = it / 2;
        // 直线型：点开 → 文件在圆钮**朝屏心方向**排成一排/一列 —— 贴上边就在
        // 图标下方一横排、贴下边在上方、贴左边在右侧一竖列、贴右边在左侧；
        // 屏幕中间（四边都够远）默认圆钮下方一横排。一排放不下先收缩间距
        //（最低 6），仍放不下朝屏心折行，保持"排"的整齐感。
        const dTop = bcy - sy;
        const dBottom = sy + sh - bcy;
        const dLeft = bcx - sx;
        const dRight = sx + sw - bcx;
        const nearTh = rb + half + 40; // 圆钮边到屏边不足此值视为"贴边"
        const minD = Math.min(dTop, dBottom, dLeft, dRight);
        // [nx, ny]：法线方向（圆钮 → 文件排），必须是严格单位向量；
        // 贴哪条边就朝屏心；不贴边（四边都够远）默认圆钮下方一横排
        let nx = 0;
        let ny = 1;
        if (minD < nearTh) {
          if (minD === dLeft) {
            nx = 1;
            ny = 0;
          } else if (minD === dRight) {
            nx = -1;
            ny = 0;
          } else if (minD === dTop) {
            nx = 0;
            ny = 1;
          } else {
            nx = 0;
            ny = -1;
          }
        }
        const horiz = ny !== 0; // 横排（沿 x 排开）还是竖列（沿 y 排开）
        const avail = horiz ? sw - 2 * PAD - it : sh - 2 * PAD - it; // 主轴可用长度
        let g = Math.max(10, gap);
        if ((cnt - 1) * (it + g) > avail) {
          g = Math.max(6, (avail - cnt * it) / Math.max(1, cnt - 1));
        }
        const oneLine = (cnt - 1) * (it + g) <= avail;
        const lineDist = rb + half + g; // 法线净距（圆钮边 → 文件边 = g）≥ clear
        const ptsL: [number, number][] = [];
        if (oneLine) {
          const span = (cnt - 1) * (it + g);
          for (let i = 0; i < cnt; i++) {
            const t = -span / 2 + half + i * (it + g);
            ptsL.push(horiz ? [t, ny * lineDist] : [nx * lineDist, t]);
          }
        } else {
          // 折行/折列：每行尽量多，行距 it+g，朝屏心逐行推进
          const per = Math.max(1, Math.floor((avail + g) / (it + g)));
          const rows = Math.ceil(cnt / per);
          for (let r = 0; r < rows; r++) {
            const c0 = r * per;
            const m = Math.min(cnt, c0 + per) - c0;
            const span = (m - 1) * (it + g);
            const dist = lineDist + r * (it + g);
            for (let i = 0; i < m; i++) {
              const t = -span / 2 + i * (it + g);
              ptsL.push(horiz ? [t, ny * dist] : [nx * dist, t]);
            }
          }
        }
        offs = ptsL;
      } else {
        const R = radialRingRadius(cnt);
        offs = Array.from({ length: cnt }, (_, i) => {
          const a = ((-90 + (360 / cnt) * i) * Math.PI) / 180;
          return [Math.cos(a) * R, Math.sin(a) * R] as [number, number];
        });
      }
      // 第一选择：环心 = 圆钮（v=0）；越界就把 v 往屏内收（每轴取最小位移）
      const minOx = Math.min(...offs.map((p) => p[0]));
      const maxOx = Math.max(...offs.map((p) => p[0]));
      const minOy = Math.min(...offs.map((p) => p[1]));
      const maxOy = Math.max(...offs.map((p) => p[1]));
      const loX = sx + PAD + it / 2 - bcx - minOx;
      const hiX = sx + sw - PAD - it / 2 - bcx - maxOx;
      const loY = sy + PAD + ih - bcy - minOy;
      const hiY = sy + sh - PAD - ih - bcy - maxOy;
      const vx = Math.min(hiX, Math.max(loX, 0));
      const vy = Math.min(hiY, Math.max(loY, 0));
      // 护栏：平移后最近文件与圆钮仍要有净距；屏幕比内容还小 → 走扇形
      const fits =
        loX <= hiX &&
        loY <= hiY &&
        offs.every(([ox, oy]) => Math.hypot(ox + vx, oy + vy) >= clear);
      let pts: [number, number][];
      if (fits) {
        pts = offs.map(([ox, oy]) => [ox + vx, oy + vy] as [number, number]);
      } else {
        // —— 弧形模式（屏幕边缘优雅展开）——
        // 圆钮贴近屏幕边缘、整环绕不开时：**自然环半径保持不动**，把文件均匀
        // 铺在"看得见的方向"上 —— 图标贴上边就呈约 2/3 个圆（朝屏内的弧段），
        // 朝屏外的那 1/3 不排文件。绝不压半径去硬塞，避免文件挤在一起。
        const th0 = Math.atan2(sy + sh / 2 - bcy, sx + sw / 2 - bcx);
        // 沿 th 方向的射线到屏边的最大距离（扣掉文件半边长，即文件中心允许的距离）
        const rayMax = (th: number) => {
          const c = Math.cos(th);
          const s2 = Math.sin(th);
          const tx =
            c > 1e-6 ? (sx + sw - PAD - bcx) / c : c < -1e-6 ? (sx + PAD - bcx) / c : Infinity;
          const ty =
            s2 > 1e-6 ? (sy + sh - PAD - bcy) / s2 : s2 < -1e-6 ? (sy + PAD - bcy) / s2 : Infinity;
          return Math.min(tx, ty) - it / 2;
        };
        const R0 = radialRingRadius(cnt);
        // 在阈值 T 下，从朝屏心方向 th0 向两侧扫描出连续"放得下"的角度窗 [t1, t2]
        const scan = (T: number): [number, number] => {
          const STEP = Math.PI / 90; // 2°
          let lo = 0;
          let hi = 0;
          for (let t = STEP; t <= Math.PI + 1e-9; t += STEP) {
            if (rayMax(th0 + t) >= T) hi = t;
            else break;
          }
          for (let t = STEP; t <= Math.PI + 1e-9; t += STEP) {
            if (rayMax(th0 - t) >= T) lo = t;
            else break;
          }
          return [th0 - lo, th0 + hi];
        };
        let R = R0;
        let [t1, t2] = scan(R);
        // 弧两端各让出"文件半边长"对应的角度余量（探头别顶着屏幕边缘）
        const margin = () =>
          Math.min(
            Math.max(t2 - t1, 0) * 0.22,
            Math.asin(Math.min(1, (it / 2 + 2) / Math.max(R, 1)))
          );
        // 弧上相邻文件的最小弦长约束：弦 ≥ it+gap 才不挤；自然半径不够就抬半径重扫
        const chordR = (span: number) =>
          cnt <= 1 ? 0 : (it + gap) / (2 * Math.sin(Math.min(Math.PI / 2, span / (2 * (cnt - 1)))));
        const rMin = chordR(Math.max(0.12, t2 - t1 - 2 * margin()));
        if (rMin > R) {
          const R2 = Math.min(rMin, Math.max(rayMax(th0), clear));
          const w = scan(R2);
          if (w[1] - w[0] > 0.15) {
            R = R2;
            t1 = w[0];
            t2 = w[1];
          }
          // 抬不动（窗口反而更窄）就维持自然半径的原窗口 —— 略挤好过跑出屏幕
        }
        const m = margin();
        const a0 = t1 + m;
        const a1 = t2 - m;
        pts =
          cnt <= 1
            ? [
                [
                  Math.cos(Math.min(a1, Math.max(a0, th0))) * R,
                  Math.sin(Math.min(a1, Math.max(a0, th0))) * R,
                ] as [number, number],
              ]
            : Array.from({ length: cnt }, (_, i) => {
                const th = a0 + ((a1 - a0) * i) / (cnt - 1);
                return [Math.cos(th) * R, Math.sin(th) * R] as [number, number];
              });
      }
      // 窗口 = 圆钮本体 + 所有文件方块的外接矩形 + 边距；圆钮在窗口内的位置随之得出
      let x0 = -rb;
      let x1 = rb;
      let y0 = -rb;
      let y1 = rb;
      for (const [ox, oy] of pts) {
        x0 = Math.min(x0, ox - it / 2);
        x1 = Math.max(x1, ox + it / 2);
        y0 = Math.min(y0, oy - ih);
        y1 = Math.max(y1, oy + ih);
      }
      return {
        pts,
        win: [Math.ceil(x1 - x0 + 2 * M), Math.ceil(y1 - y0 + 2 * M)] as [number, number],
        btn: [Math.round(M - x0), Math.round(M - y0)] as [number, number],
      };
    },
    [radialPref.ringGap, radialPref.itemSize, radialPref.size, radialRectLayout, radialRingRadius]
  );

  /** 单击圆钮 → 弹出。DOM 先切（stage 绽放动画起跑），窗口几何由下方 effect
   *  统一落位；圆钮屏幕位置记入 ref，全程钉死不动。展开期间窗口置顶（悬浮），
   *  文件浮在其它分区之上可以点击；收回时归还桌面层。 */
  const expandRadial = useCallback(() => {
    if (tempExpanded || radialClosing) return;
    const win = getCurrentWindow();
    // currentMonitor：窗口所在显示器（含工作区 = 整屏 - 任务栏），物理像素
    Promise.all([win.outerSize(), win.outerPosition(), currentMonitor().catch(() => null)])
      .then(([s, p, mon]) => {
        radialSavedRect.current = [p.x, p.y, s.width, s.height];
        const k = s.width / Math.max(1, window.innerWidth); // 物理 / css 像素比
        radialKRef.current = k;
        radialBtnCss.current = [(p.x + s.width / 2) / k, (p.y + s.height / 2) / k];
        if (mon) {
          radialWorkArea.current = {
            x: mon.workArea.position.x / k,
            y: mon.workArea.position.y / k,
            w: mon.workArea.size.width / k,
            h: mon.workArea.size.height / k,
          };
        }
        setTempExpanded(true);
        // 展开瞬间置顶：文件要浮在其它分区之上（NOACTIVATE，不抢焦点）
        invoke("set_radial_floating", { float: true }).catch(() => {});
      })
      .catch(() => {});
  }, [tempExpanded, radialClosing]);

  // 展开态窗口几何：按「圆钮屏幕位置不变」算布局，两帧后一次落位
  // （DOM 绽放先起跑、窗口随后放大，避免闪大圆）。items 增删（拖入/拖出）
  // 自动重算重放：圆钮钉在原地，文件重排进空余处。
  useEffect(() => {
    if (!isRadial || !tempExpanded || radialClosing) return;
    const st = cfgRef.current.radialStyle;
    const style: "pie" | "grid" | "rect" | "line" =
      st === "grid" || st === "rect" || st === "line" ? st : "pie";
    const names = cfgRef.current.radialShowNames !== false;
    const lay = computeRadialLayout(style, items.length, names);
    const win = getCurrentWindow();
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const k = radialKRef.current || 1;
        const tw = Math.round(lay.win[0] * k);
        const th = Math.round(lay.win[1] * k);
        const [bcx, bcy] = radialBtnCss.current;
        const fx = Math.round(bcx * k - lay.btn[0] * k);
        const fy = Math.round(bcy * k - lay.btn[1] * k);
        win.setSize(new PhysicalSize(tw, th)).catch(() => {});
        win.setPosition(new PhysicalPosition(fx, fy)).catch(() => {});
        // 落位后再补一次置顶：展开瞬间的置顶可能与落位竞争，
        // 部分环境下被随后的一次 z 序操作顶下去（双保险）
        invoke("set_radial_floating", { float: true }).catch(() => {});
      })
    );
  }, [isRadial, tempExpanded, radialClosing, items.length, computeRadialLayout]);

  /** 收回圆钮：stage 先播 180ms CSS 收拢 → **先把窗口缩回原矩形、再切回圆钮
   *  DOM**（顺序反了圆钮 div 会 100% 铺满大窗口，又闪一次大圆） */
  const collapseRadial = useCallback(() => {
    if (radialClosing) return;
    setRadialClosing(true);
    setBtnHover(false); // 悬停角标随收回立即隐藏，不残留
    window.setTimeout(() => {
      const win = getCurrentWindow();
      const saved = radialSavedRect.current;
      const shrink = saved
        ? Promise.all([
            win.setSize(new PhysicalSize(saved[2], saved[3])),
            win.setPosition(new PhysicalPosition(saved[0], saved[1])),
          ]).catch(() => {})
        : win
            .scaleFactor()
            .then((sf) => {
              const d = Math.round(radialPref.size * sf);
              return win.setSize(new PhysicalSize(d, d));
            })
            .catch(() => {});
      shrink.then(() => {
        setRadialClosing(false);
        setTempExpanded(false);
        // 收回完成：归还桌面层（去置顶 + 沉回桌面组之上）
        invoke("set_radial_floating", { float: false }).catch(() => {});
        // 嵌套的饼型分区：收回后重新藏进宿主分区（回列表条目，不留在桌面）
        if ((box?.nestedIn ?? 0) !== 0) {
          hideNestedRadial(boxId).catch(() => {});
        }
      });
    }, 140);
  }, [radialClosing, radialPref.size, box?.nestedIn, boxId]);

  // 饼型文件全被拖走 → 舞台空了，自动收回成圆钮
  useEffect(() => {
    if (tempExpanded && isRadial && items.length === 0) collapseRadial();
  }, [items.length, tempExpanded, isRadial, collapseRadial]);

  // 嵌套的饼型分区：宿主分区里点了圆钮条目 → 后端把窗口挪到条目旁并显示，
  // 这里收到事件后刷新一次文件列表再弹出展开环（保证条目是最新的）
  useEffect(() => {
    if (!isRadial) return;
    const un = listen<number>("nested-radial-open", () => {
      Promise.resolve(load())
        .then(() => expandRadial())
        .catch(() => {});
    });
    return () => {
      un.then((f) => f());
    };
  }, [isRadial, load, expandRadial]);

  // 饼型圆钮尺寸校正：窗口物理直径必须等于 设置直径 × scale factor。
  // 后端建窗用的是默认 54；用户在设置页改了直径后，靠这里把所有圆钮窗口
  // 校到位（settings-changed → load() → radialPref 变化 → 本 effect）。
  // 延迟 300ms 执行：躲开收回补间动画（120ms），避免两路 set_size 打架。
  useEffect(() => {
    if (!isRadial || tempExpanded || !cfg.collapsed) return;
    let dead = false;
    const t = window.setTimeout(() => {
      const win = getCurrentWindow();
      Promise.all([win.outerSize(), win.scaleFactor()])
        .then(([s, sf]) => {
          if (dead) return;
          const want = Math.round(radialPref.size * sf);
          if (Math.abs(s.width - want) > 1 || Math.abs(s.height - want) > 1) {
            win.setSize(new PhysicalSize(want, want)).catch(() => {});
          }
        })
        .catch(() => {});
    }, 300);
    return () => {
      dead = true;
      window.clearTimeout(t);
    };
  }, [isRadial, tempExpanded, cfg.collapsed, radialPref.size]);

  // 临时展开期间：鼠标真正离开窗口（relatedTarget 为空）才开始收起倒计时，
  // 回到窗口内或按下鼠标都取消 —— "交互中绝不收起"，移出了才收。
  // 饼型菜单分区同样适用：移出 0.15s 自动收回（要"跟手利落"的感觉）。
  useEffect(() => {
    if (!tempExpanded) return;
    const startClose = () => {
      if (hoverCloseTimer.current != null) return;
      hoverCloseTimer.current = window.setTimeout(() => {
        hoverCloseTimer.current = null;
        if (isRadial) {
          collapseRadial();
        } else {
          doHoverCollapse();
        }
      }, 150);
    };
    const cancelClose = () => {
      if (hoverCloseTimer.current != null) {
        window.clearTimeout(hoverCloseTimer.current);
        hoverCloseTimer.current = null;
      }
    };
    const onOut = (e: MouseEvent) => {
      if (!e.relatedTarget) startClose();
    };
    const onOver = () => cancelClose();
    const onDown = () => cancelClose();
    document.addEventListener("mouseout", onOut);
    document.addEventListener("mouseover", onOver);
    document.addEventListener("pointerdown", onDown);
    return () => {
      document.removeEventListener("mouseout", onOut);
      document.removeEventListener("mouseover", onOver);
      document.removeEventListener("pointerdown", onDown);
      cancelClose();
    };
  }, [tempExpanded, isRadial, collapseRadial, doHoverCollapse]);

  /** 点击胶囊：永久展开（几何由后端恢复），完成后按数据库真值刷新 */
  const expandPermanently = useCallback(() => {
    clearHoverTimers();
    cancelSizeAnim();
    setTempExpanded(false);
    setBoxCollapsed(boxId, false)
      .then(() => load())
      .catch((e) => console.error("展开分区失败", e));
  }, [boxId, clearHoverTimers, cancelSizeAnim, load]);

  // 后端 set_box_collapsed 完成后会广播状态（展开/收起互相切换时同步本地视图）
  useEffect(() => {
    const un = listen<{ boxId: number; collapsed: boolean }>("box-capsule-changed", (e) => {
      if (e.payload.boxId !== boxId) return;
      // 后端已直接设定窗口尺寸，前端补间必须立即停手，否则两路 set_size 打架
      cancelSizeAnim();
      if (e.payload.collapsed) {
        clearHoverTimers();
        setTempExpanded(false);
      }
      load();
    });
    return () => {
      un.then((f) => f());
    };
  }, [boxId, load, clearHoverTimers, cancelSizeAnim]);

  useEffect(() => {
    if (!editing) return;
    requestKeyboardFocus();
    editRef.current?.focus();
    editRef.current?.select();
  }, [editing, requestKeyboardFocus]);

  const updateCfg = useCallback(
    (patch: Partial<BoxWindowCfg>) => {
      // 副作用放在 setState updater 外面：严格模式下 updater 会跑两次，
      // 写在里面等于一次操作发两次落盘 IPC。
      const next = { ...cfgRef.current, ...patch };
      // cfg 镜像一起更新：拖放重排等回调要读到最新值
      cfgRef.current = next;
      setCfg(next);
      saveBoxWindowCfg(next).catch(() => {});
    },
    []
  );

  /** 落点（物理像素）落在第几个条目上；-1 = 空白处 */
  const indexAtPoint = useCallback((x: number, y: number) => {
    // 物理像素 → CSS 像素，再找那个点上是谁
    const dpr = window.devicePixelRatio || 1;
    const hit = document.elementFromPoint(x / dpr, y / dpr);
    const node = hit?.closest("[data-idx]") as HTMLElement | null;
    return node ? Number(node.dataset.idx) : -1;
  }, []);

  /**
   * 把某个图标"穿插"到第 t 格（后端把落点坐标转成事件发过来）。
   *
   * 关键：**先把自己摘出去、再插进去** —— 目标位置在它后面时下标要减 1，
   * 不减就会多挪一格，这就是之前"位置会乱"的原因。
   */
  const reorderTo = useCallback((path: string, t: number) => {
    const cur = itemsRef.current;
    const from = cur.findIndex((i) => i.path === path);
    if (from < 0) return;
    // 落点是空白处（t < 0）：保持原样。原来会当成"放到最后一个"，
    // 于是把图标拖到分区空白处松手，它就莫名其妙跑到末尾去了。
    if (!Number.isFinite(t) || t < 0) return;
    const dropIdx = Math.min(t, cur.length - 1);
    const insertAt = dropIdx > from ? dropIdx - 1 : dropIdx;
    if (insertAt === from) return;
    const next = [...cur];
    const [moved] = next.splice(from, 1);
    next.splice(insertAt, 0, moved);
    itemsRef.current = next;
    setItems(next);
  }, []);

  // 拖动经过本分区：实时把图标穿插到鼠标停留的那一格，松手前就能看到最终位置
  useEffect(() => {
    const un = listen<{ boxId: number; x: number; y: number }>("box-item-hover", (e) => {
      const info = dragInfo.current;
      const { boxId: targetBox, x, y } = e.payload;
      // x < 0 = 已经拖出分区，保持现状别动
      if (!info || targetBox !== boxId || x < 0) return;
      reorderTo(info.path, indexAtPoint(x, y));
    });
    return () => {
      un.then((f) => f());
    };
  }, [boxId, indexAtPoint, reorderTo]);

  // 松手落在本分区 → 当前顺序就是最终顺序，记进分区配置
  useEffect(() => {
    const un = listen<{ boxId: number; path: string; x: number; y: number }>(
      "box-item-drop",
      (e) => {
        const { boxId: targetBox, path, x, y } = e.payload;
        if (targetBox !== boxId) return;
        if (dragInfo.current) dragInfo.current.dropped = true;
        // 兜底：拖得飞快时可能没收到悬停事件，按落点再穿插一次
        reorderTo(path, indexAtPoint(x, y));
        updateCfg({ itemOrder: itemsRef.current.map((i) => i.path) });
      }
    );
    return () => {
      un.then((f) => f());
    };
  }, [boxId, indexAtPoint, reorderTo, updateCfg]);

  // ---------- 拖动（纯手动屏幕坐标，避免系统模态拖动导致卡死） ----------
  const dragState = useRef<{
    sx: number;
    sy: number;
    wx: number;
    wy: number;
    /** 窗口尺寸（物理像素），磁性吸附要用 */
    ww: number;
    wh: number;
    /** 磁性吸附候选 [x,y,w,h] + 阈值（物理像素） */
    targets: [number, number, number, number][];
    threshold: number;
    scale: number;
    moved: boolean;
    active: boolean;
    lastX: number;
    lastY: number;
  } | null>(null);
  const rafRef = useRef<number | null>(null);

  const onHeaderDown = useCallback(
    (e: React.PointerEvent) => {
      if (cfg.locked || (e.target as HTMLElement).closest("button, input")) return;
      if (e.button !== 0) return;
      if (dragState.current?.active) return;
      const win = getCurrentWindow();
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      e.preventDefault();
      // 不调用 setFocus：分区窗口带 WS_EX_NOACTIVATE（点击不激活、不置顶），
      // 拖动靠 pointer capture 完成，不需要系统焦点；强行置顶会让所有分区闪一下
      Promise.all([
        win.outerPosition(),
        win.outerSize(),
        win.scaleFactor().catch(() => 1),
        getBoxSnapTargets(boxId).catch(() => ({ rects: [], threshold: 0 })),
      ])
        .then(([pos, sz, scale, snap]) => {
          dragState.current = {
            sx: e.screenX,
            sy: e.screenY,
            wx: pos.x,
            wy: pos.y,
            ww: sz.width,
            wh: sz.height,
            targets: snap.rects,
            threshold: snap.threshold,
            scale: typeof scale === "number" && scale > 0 ? scale : 1,
            moved: false,
            active: true,
            lastX: e.screenX,
            lastY: e.screenY,
          };
        })
        .catch(() => {});
    },
    [cfg.locked, boxId]
  );

  const onHeaderMove = useCallback((e: React.PointerEvent) => {
    const ds = dragState.current;
    if (!ds?.active) return;
    ds.lastX = e.screenX;
    ds.lastY = e.screenY;
    const dx = (e.screenX - ds.sx) * ds.scale;
    const dy = (e.screenY - ds.sy) * ds.scale;
    if (Math.abs(dx) > 2 || Math.abs(dy) > 2) ds.moved = true;
    if (!ds.moved) return;
    // rAF 节流，避免 IPC 风暴
    if (rafRef.current != null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      const s = dragState.current;
      if (!s?.active) return;
      let nx = s.wx + Math.round((s.lastX - s.sx) * s.scale);
      let ny = s.wy + Math.round((s.lastY - s.sy) * s.scale);
      // 拖动过程实时磁性吸附：边缘一靠近候选边就"粘住"，
      // 继续往反方向拖超过阈值才会挣脱（绝对坐标计算，不会累积漂移）
      if (s.threshold > 0 && s.targets.length > 0) {
        [nx, ny] = magnetSnap(nx, ny, s.ww, s.wh, s.targets, s.threshold);
      }
      getCurrentWindow()
        .setPosition(new PhysicalPosition(nx, ny))
        .catch(() => {});
    });
  }, [magnetSnap]);

  const onHeaderUp = useCallback(
    (e: React.PointerEvent) => {
      const ds = dragState.current;
      if (!ds) return;
      dragState.current = null;
      if (rafRef.current != null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
      try {
        e.currentTarget.releasePointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      if (ds.moved) {
        // 饼型圆钮的落点处理（嵌套 / 普通拖动落位）由圆钮自己的 pointerup 做
        if (box?.boxType === "radial") return;
        startBoxDrag(boxId, true).catch(() => {});
      }
    },
    [boxId, box?.boxType]
  );

  /**
   * 标题双击重命名。
   * 头部在 pointerdown 里 preventDefault + setPointerCapture 来实现拖动，
   * 浏览器因此不会派发 click/dblclick，原生 onDoubleClick 永远不会触发，
   * 所以在 pointerdown 里自己按「时间 + 位置」判定双击。
   */
  const onTitleDown = useCallback(
    (e: React.PointerEvent) => {
      if (e.button !== 0) return;
      const now = performance.now();
      const prev = titleTap.current;
      if (
        prev &&
        now - prev.t < 400 &&
        Math.abs(e.screenX - prev.x) < 8 &&
        Math.abs(e.screenY - prev.y) < 8
      ) {
        titleTap.current = null;
        // 拦住头部，避免第二下按下后移动鼠标把窗口拖着走
        e.preventDefault();
        e.stopPropagation();
        dragState.current = null;
        setEditName(box?.name || "");
        setEditing(true);
        return;
      }
      titleTap.current = { t: now, x: e.screenX, y: e.screenY };
    },
    [box?.name]
  );

  // ---------- 调整大小（纯手动，避免系统模态 resize 卡死） ----------
  // 拉伸时带吸附：右/下边缘靠近其它分区（或屏幕工作区）的边就对齐
  const resizeState = useRef<{
    sx: number;
    sy: number;
    w: number;
    h: number;
    scale: number;
    /** 本窗口左上角（拉伸时不动，右/下边缘由它 + 新宽高决定） */
    px: number;
    py: number;
    /** 候选矩形 [x,y,w,h]，物理像素 */
    targets: [number, number, number, number][];
    threshold: number;
    /** 最新鼠标位置：rAF 回调里读它，避免用过期的 pointermove 事件 */
    lastX: number;
    lastY: number;
  } | null>(null);
  const resizeRaf = useRef<number | null>(null);

  const onResizeDown = useCallback(
    (e: React.PointerEvent) => {
      if (cfg.locked) return;
      e.stopPropagation();
      e.preventDefault();
      const win = getCurrentWindow();
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      Promise.all([
        win.outerSize(),
        win.scaleFactor().catch(() => 1),
        win.outerPosition(),
        getBoxSnapTargets(boxId).catch(() => ({ rects: [], threshold: 0 })),
      ])
        .then(([sz, scale, pos, snap]) => {
          resizeState.current = {
            sx: e.screenX,
            sy: e.screenY,
            w: sz.width,
            h: sz.height,
            scale: typeof scale === "number" && scale > 0 ? scale : 1,
            px: pos.x,
            py: pos.y,
            targets: snap.rects,
            threshold: snap.threshold,
            lastX: e.screenX,
            lastY: e.screenY,
          };
        })
        .catch(() => {});
    },
    [cfg.locked, boxId]
  );

  const onResizeMove = useCallback((e: React.PointerEvent) => {
    const rs = resizeState.current;
    if (!rs) return;
    // 每次移动都记最新坐标，rAF 回调里用它（别用捕获的过期事件）
    rs.lastX = e.screenX;
    rs.lastY = e.screenY;
    if (resizeRaf.current != null) return;
    resizeRaf.current = requestAnimationFrame(() => {
      resizeRaf.current = null;
      const s = resizeState.current;
      if (!s) return;
      let nw = Math.max(140, Math.round(s.w + (s.lastX - s.sx) * s.scale));
      let nh = Math.max(100, Math.round(s.h + (s.lastY - s.sy) * s.scale));
      // ---- 拉伸吸附：右/下边缘在阈值内就贴到候选边上（x/y 两轴独立） ----
      if (s.threshold > 0 && s.targets.length > 0) {
        [nw, nh] = resizeSnap(s.px, s.py, nw, nh, s.targets, s.threshold);
        nw = Math.max(140, nw);
        nh = Math.max(100, nh);
      }
      getCurrentWindow()
        .setSize(new PhysicalSize(nw, nh))
        .catch(() => {});
    });
  }, []);

  const onResizeUp = useCallback((e: React.PointerEvent) => {
    resizeState.current = null;
    if (resizeRaf.current != null) {
      cancelAnimationFrame(resizeRaf.current);
      resizeRaf.current = null;
    }
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* ignore */
    }
  }, []);

  // ---------- 动作 ----------
  const openItem = useCallback((it: IconItem) => {
    // 嵌套饼型分区条目不是文件（path 是 box:{id} 虚拟值），单击展开走 openNestedEntry
    if (it.nestedBoxId) return;
    // 走后端 ShellExecute（自己的命令，不受 opener 插件权限限制）。
    // 失败不要静默：把原因打到控制台，避免再出现"双击没反应"却无从排查
    openFile(it.path).catch((e) => console.error("打开文件失败", it.path, e));
  }, []);

  // ---------- 就地改名（右键菜单 → 重命名） ----------

  const startRename = useCallback((it: IconItem) => {
    const r = { path: it.path, name: it.name };
    renamingRef.current = r;
    setRenaming(r);
  }, []);

  /** 退出改名状态；提交和取消都先走它，保证只会真正执行一次 */
  const endRename = useCallback(() => {
    renamingRef.current = null;
    setRenaming(null);
  }, []);

  /** 回车 / 失焦提交。名字没变就什么都不做；后端改完会通知各分区重读列表 */
  const commitRename = useCallback(() => {
    const r = renamingRef.current;
    endRename();
    if (!r) return;
    const name = r.name.trim();
    const orig = r.path.split(/[\\/]/).pop();
    if (!name || name === orig) return;
    renameFile(r.path, name).catch((e) => console.error("重命名失败", r.path, e));
  }, [endRename]);

  /** 文件右击：弹系统原生菜单；用户选了菜单里的「重命名」就转入就地改名。
   *
   *  其余命令（删除到回收站 / 剪切 / 粘贴 / 发送到…）都由 Shell 直接执行，
   *  执行完目录已经变了，**必须就地刷新列表** —— 以前这里不刷新，图标要等下一次
   *  交互（右键空白处、拖放、收到后端事件）才消失，看起来就是"删了还在"。 */
  const onItemMenu = useCallback(
    (e: React.MouseEvent, it: IconItem) => {
      e.preventDefault();
      e.stopPropagation();
      // 嵌套饼型分区条目：弹自家小菜单（移出到桌面），不是文件、没有系统菜单
      if (it.nestedBoxId) {
        const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
        setNestedMenu({
          x: rect.left,
          y: rect.bottom + 4,
          id: it.nestedBoxId,
          name: it.name,
        });
        return;
      }
      showFileMenu(it.path)
        .then((res) => {
          if (res === "rename") {
            // 就地改名：列表内容没变，改完由后端通知刷新
            startRename(it);
            return;
          }
          refreshAfterShell();
        })
        .catch((err) => console.error("弹出右键菜单失败", it.path, err));
    },
    [startRename, refreshAfterShell]
  );

  /** 嵌套饼型分区条目的小菜单（右键）：目前只有「移出到桌面」 */
  const [nestedMenu, setNestedMenu] = useState<{
    x: number;
    y: number;
    id: number;
    name: string;
  } | null>(null);

  /** 单击嵌套条目 → 在条目处弹出该饼型分区的展开环 */
  const openNestedEntry = useCallback(
    (e: React.MouseEvent, it: IconItem) => {
      const id = it.nestedBoxId;
      if (!id) return;
      const win = getCurrentWindow();
      const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
      Promise.all([win.outerPosition(), win.scaleFactor().catch(() => 1)])
        .then(([pos, sf]) => {
          const k = typeof sf === "number" && sf > 0 ? sf : 1;
          // 条目中心的物理屏幕坐标 = 窗口左上角 + 条目中心(CSS px) × 缩放
          const cx = pos.x + Math.round((rect.left + rect.width / 2) * k);
          const cy = pos.y + Math.round((rect.top + rect.height / 2) * k);
          showNestedRadial(id, cx, cy).catch((err) => showToast(errText(err), true));
        })
        .catch(() => {
          // 拿不到窗口几何就退化为光标位置（后端会夹回屏幕内）
          showNestedRadial(id, Math.round(e.screenX), Math.round(e.screenY)).catch(() => {});
        });
    },
    [showToast]
  );

  /** 小菜单里的「移出到桌面」：恢复成独立圆钮 */
  const unnestEntry = useCallback(
    (id: number) => {
      setNestedMenu(null);
      unnestRadialBox(id)
        .then(() => showToast("已移出到桌面", false))
        .catch((err) => showToast(errText(err), true));
    },
    [showToast]
  );

  // 小菜单开着时：点别处 / Esc 都收起
  useEffect(() => {
    if (!nestedMenu) return;
    const close = () => setNestedMenu(null);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.setTimeout(() => {
      document.addEventListener("pointerdown", close);
    }, 0);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", onKey);
    };
  }, [nestedMenu]);

  /** 空白处右击：弹系统原生背景菜单（新建 / 在资源管理器中打开 / 属性 …），
   *  和资源管理器里打开文件夹后空白处右键基本一致 */
  const onBackgroundMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      showBoxBackgroundMenu(boxId)
        .then(() => load())
        .catch((err) => {
          console.error("弹出背景右键菜单失败", boxId, err);
          // 用窗口内的轻提示，不要再弹系统弹框：右击这种随手动作，
          // 失败时弹一个 "操作失败：<底层错误>" 的模态框非常吓人
          showToast(errText(err), true);
        });
    },
    [boxId, load, showToast]
  );

  // 输入框出现时聚焦，并像资源管理器那样只选中主名（不含扩展名）
  useEffect(() => {
    const el = renameInputRef.current;
    if (!el || !renaming) return;
    requestKeyboardFocus();
    el.focus();
    const dot = renaming.name.lastIndexOf(".");
    el.setSelectionRange(0, dot > 0 ? dot : renaming.name.length);
    // 只在切换到另一个文件时定位，输入过程中不要重置选区
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renaming?.path]);

  /** 改名输入框（网格 / 列表两个视图共用） */
  const renderRenameInput = (it: IconItem, extraClass: string) => (
    <input
      ref={renameInputRef}
      className={`box-win-rename ${extraClass}`}
      value={renaming?.name ?? ""}
      spellCheck={false}
      onChange={(e) => {
        const next = { path: it.path, name: e.target.value };
        renamingRef.current = next;
        setRenaming(next);
      }}
      // 别让这些事件冒到条目上 —— 否则会触发拖动 / 打开
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onBlur={commitRename}
      onKeyDown={(e) => {
        if (e.key === "Enter") commitRename();
        else if (e.key === "Escape") endRename();
      }}
    />
  );

  /**
   * 按住文件图标拖动超过阈值 → 启动系统原生拖拽。
   *
   * 不用 HTML5 drag：Tauri 的拖放通道接管了 OS 级拖放，而且只有原生 OLE 拖拽
   * 拖出去的是**真文件**，才能落到桌面 / 资源管理器 / 其它分区 / 别的程序里。
   * 6px 阈值是为了不影响"双击打开"。
   */
  const beginFileDrag = useCallback(
    (e: React.PointerEvent, path: string) => {
      if (e.button !== 0) return;
      // 嵌套饼型分区条目是虚拟条目（box:{id}），没有真文件可拖
      if (path.startsWith("box:")) return;
      const sx = e.screenX;
      const sy = e.screenY;
      let started = false;
      const finish = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        window.removeEventListener("pointercancel", onUp);
      };
      const onMove = (ev: PointerEvent) => {
        if (started) return;
        if (Math.abs(ev.screenX - sx) < 6 && Math.abs(ev.screenY - sy) < 6) return;
        started = true;
        // 记下"拖的是谁"和"拖动前的顺序"：落在别处时要把临时穿插还原回去
        dragInfo.current = { path, snapshot: itemsRef.current, dropped: false };
        // 命令在松开鼠标后才 resolve，所以在这里收尾最合适。
        // boxId 一起传：后端靠它排除"落回自己分区"，并在落点事件没送到时兜底。
        startFileDrag(path, boxId)
          .catch((err) => console.error("拖拽文件失败", path, err))
          .finally(() => {
            const info = dragInfo.current;
            dragInfo.current = null;
            if (info && !info.dropped) {
              // 没落在本分区（拖去桌面 / 别的程序了）→ 撤销拖动过程中的临时排序
              itemsRef.current = info.snapshot;
              setItems(info.snapshot);
            }
            finish();
          });
      };
      const onUp = () => finish();
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    },
    [boxId]
  );

  const doOrganize = useCallback(async () => {
    try {
      const r = await organizeBox(boxId);
      // 以前失败直接吞掉（"我点了整理但没反应"）。现在给出数量和第一条原因。
      if (r.failed > 0) {
        const first = r.errors?.[0] ?? "";
        showToast(
          `${r.moved} 个已移出，${r.failed} 个失败${first ? `：${first}` : ""}`,
          true
        );
      } else if (r.moved > 0) {
        showToast(`已移出 ${r.moved} 个不匹配文件`, false);
      }
      load();
    } catch (e) {
      showToast(`整理失败：${errText(e)}`, true);
    }
  }, [boxId, load, showToast]);

  /**
   * 解散分区 —— **一步到位，不再有任何确认弹窗**。
   *
   * 语义只有一种：「分组解散，文件原来从哪里来的就回哪里去」。
   * - 普通分区：分区里的文件按搬运记录退回整理前的**原始位置**（原本在桌面就回桌面，
   *   原本在某个文件夹里就回那个文件夹；重名自动加序号）。文件本身只是被搬回去，
   *   仍是原来的图标 / 快捷方式，不改名、不改属性、不删除。
   * - 关联文件夹分区：链接的是用户自己的目录，后端连碰都不碰，只撤掉这块面板。
   *
   * 以前这里要先弹一个确认框、再让用户二选一「移回桌面 / 留在原文件夹」——
   * 多两步没有任何收益，还容易点错。现在直接走"退回原位"。
   */
  const doDelete = useCallback(async () => {
    if (!box) return;
    closeBoxMenu(boxId).catch(() => {});
    try {
      await deleteBox(boxId, true);
    } catch (e) {
      // 解散失败（还有文件被别的程序占用等）必须说出来，不能静默吞掉 ——
      // 否则用户以为分区已经解散了，其实一切都还在。
      showToast(`解散分区失败：${errText(e)}`, true);
      load();
    }
  }, [box, boxId, showToast, load]);

  const doRename = useCallback(async () => {
    const name = editName.trim();
    if (name && name !== box?.name) {
      try {
        await renameBox(boxId, name);
        load();
      } catch {
        /* ignore */
      }
    }
    setEditing(false);
  }, [editName, box, boxId, load]);

  // 新建同类型分区。
  // 名字交给后端取（传空串）：以前前端按"同类型分区个数 + 1"自己拼，
  // 7 个关联文件夹分区算出下一个叫"文件夹 8"，而库里恰好已经有个"文件夹 8"，
  // 后端直接报"已存在同名分区"，前端又只 console.error —— 表现就是
  // "点新建分区毫无反应"。现在后端会自动取下一个空号，并把最终名字回给前端。
  const doCreateBox = useCallback(
    async (type: "software" | "file" | "folder" | "radial") => {
      try {
        const name = await createBox("", "#15b6c9", type);
        showToast(`已新建分区「${name}」`, false);
      } catch (e) {
        showToast(`新建分区失败：${errText(e)}`, true);
      }
    },
    [showToast]
  );

  // 链接文件夹：弹出系统文件夹选择器（新建 folder 分区 或 更改当前分区链接）
  const pickLinkFolder = useCallback(async () => {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择要关联的文件夹",
      });
      if (typeof picked !== "string" || !picked) return;
      if (box?.boxType === "folder") {
        await setBoxLink(boxId, picked);
        showToast("已更新关联的文件夹", false);
      } else {
        const name = await createBox("", "#15b6c9", "folder", picked);
        showToast(`已新建关联文件夹分区「${name}」`, false);
      }
      load();
    } catch (e) {
      showToast(`关联文件夹失败：${errText(e)}`, true);
    }
  }, [box, boxId, load, showToast]);

  // 新建一个"链接文件夹"分区（**始终新建**，不会改到当前分区）
  const newLinkedFolder = useCallback(async () => {
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: "选择要关联的文件夹",
      });
      if (typeof picked !== "string" || !picked) return;
      const name = await createBox("", "#15b6c9", "folder", picked);
      showToast(`已新建关联文件夹分区「${name}」`, false);
    } catch (e) {
      showToast(`新建关联文件夹分区失败：${errText(e)}`, true);
    }
  }, [showToast]);

  // 背景色/透明度（本分区独立）
  const applyBg = useCallback(
    (color: string, opacity: number) => {
      setBg({ color, opacity });
      updateCfg({ bgColor: color, bgOpacity: opacity });
    },
    [updateCfg]
  );

  // 文件名颜色（本分区独立，只有白/黑两态）
  const applyLabelColor = useCallback(
    (color: string) => {
      updateCfg({ labelColor: color });
    },
    [updateCfg]
  );

  /**
   * 丢掉"手工排列顺序"。
   * 换排序方式时必须一起丢掉：否则手工顺序会盖在排序结果上，看着像"排序没生效"。
   */
  const clearItemOrder = useCallback(() => {
    const next = { ...cfgRef.current, itemOrder: [] };
    cfgRef.current = next;
    setCfg(next);
    return saveBoxWindowCfg(next).catch(() => {});
  }, []);

  const applySort = useCallback(
    async (mode: string) => {
      try {
        // 按“修改时间”排时默认降序：最新的文件在最前，符合“刚加的文件在第一个”的预期
        const desc = mode === "time" ? true : box?.sortDesc ?? false;
        await setBoxSort(boxId, mode, desc);
        await clearItemOrder();
        load();
      } catch {
        /* ignore */
      }
    },
    [box?.sortDesc, boxId, clearItemOrder, load]
  );

  /** 排序方向：点一下升序，再点一下降序 */
  const toggleSortDir = useCallback(async () => {
    try {
      await setBoxSort(boxId, box?.sortMode || "name", !(box?.sortDesc ?? false));
      await clearItemOrder();
      load();
    } catch {
      /* ignore */
    }
  }, [box?.sortMode, box?.sortDesc, boxId, clearItemOrder, load]);

  // ---------- 吸附同类文件 ----------
  /** 分组勾选 + 「其他」输入框合并后的最终扩展名清单 */
  const absorbExts = useMemo(
    () => Array.from(new Set([...absorbSelected, ...parseExts(absorbCustom)])),
    [absorbSelected, absorbCustom]
  );

  const openAbsorb = useCallback(async () => {
    setAbsorbMsg("");
    setAbsorbOpen(true);
    try {
      const c = await getAbsorbCfg(boxId);
      // 已知分组读进勾选框，零散扩展名放进「其他」输入框，来回切换不丢配置
      setAbsorbSelected(c.exts.filter((e) => KNOWN_EXTS.has(e)));
      setAbsorbCustom(c.exts.filter((e) => !KNOWN_EXTS.has(e)).join(", "));
      setAbsorbAuto(c.auto);
    } catch {
      setAbsorbSelected([]);
      setAbsorbCustom("");
      setAbsorbAuto(false);
    }
  }, [boxId]);

  const toggleExtGroup = useCallback((exts: string[]) => {
    setAbsorbSelected((prev) => {
      const pickedAll = exts.every((e) => prev.includes(e));
      return pickedAll
        ? prev.filter((e) => !exts.includes(e))
        : Array.from(new Set([...prev, ...exts]));
    });
  }, []);

  /** 整组都选了 = all，选了一部分 = some（点一下补齐），没选 = none */
  const extGroupState = useCallback(
    (exts: string[]) => {
      const hit = exts.filter((e) => absorbSelected.includes(e)).length;
      return hit === 0 ? "none" : hit === exts.length ? "all" : "some";
    },
    [absorbSelected]
  );

  const doAbsorb = useCallback(async () => {
    if (absorbExts.length === 0) return;
    setAbsorbBusy(true);
    try {
      const r = await setAbsorbCfg(boxId, absorbExts, absorbAuto);
      setAbsorbMsg(
        r.moved > 0
          ? `已吸附 ${r.moved} 个文件${r.failed > 0 ? `，${r.failed} 个失败` : ""}`
          : r.failed > 0
            ? `${r.failed} 个文件移动失败`
            : "已保存，桌面上暂时没有可吸附的文件"
      );
      load();
      // 成功收进来时留一点时间让用户看到结果，再收起面板
      if (r.moved > 0) window.setTimeout(() => setAbsorbOpen(false), 900);
    } catch (e) {
      setAbsorbMsg(`吸附失败：${String(e)}`);
    } finally {
      setAbsorbBusy(false);
    }
  }, [absorbAuto, absorbExts, boxId, load]);

  const isList = cfg.viewMode === "list";
  /**
   * 图标单元的固定宽度（图标 + 两侧留白，给文件名留出省略号的位置）。
   * 列数交给 CSS 的 auto-fill 按**真实渲染宽度**自动算 —— 之前用 cfg.w 手算列数，
   * 而窗口拉伸后前端的 cfg.w 不会更新，结果不管拉多宽都只排 3 列。
   */
  const cellW = cfg.iconSize + 34;

  // 图标加载失败自动重试：后端排队封顶时会返回 503（no-store），
  // <img> 收到 503 不会自己重试，不补这一步被挤掉的图标就永远空白。
  // 指数退避 + 次数上限；成功加载即清零重试计数。
  const onIconLoad = (e: React.SyntheticEvent<HTMLImageElement>) => {
    delete e.currentTarget.dataset.retry;
  };
  const onIconError = (e: React.SyntheticEvent<HTMLImageElement>) => {
    const img = e.currentTarget;
    const n = Number(img.dataset.retry ?? "0");
    if (n >= 5) return;
    img.dataset.retry = String(n + 1);
    window.setTimeout(
      () => {
        img.src = `${img.src.split("#")[0]}#${n + 1}`;
      },
      250 * (n + 1) + Math.random() * 250
    );
  };

  const emptyText =
    box?.boxType === "folder"
      ? "未链接文件夹，点右上角 ⋮ 设置"
      : box?.boxType === "software"
        ? "把桌面快捷方式拖进来即可归入此分区"
        : "把文件拖进来即可归入此分区";

  const bgAlpha = Math.min(1, Math.max(0, bg.opacity / 100));
  const bgStyle = {
    "--box-bg": hexToRgba(bg.color, bgAlpha),
    "--box-bg-hover": hexToRgba(bg.color, Math.min(1, bgAlpha + 0.1)),
  } as React.CSSProperties;

  /**
   * 独立菜单窗口里点的操作，转交回本窗口执行 —— 菜单只负责"表达意图"，
   * 新建/外观/链接/排序/吸附/删除这些逻辑仍然只在分区窗口里维护一份。
   */
  useEffect(() => {
    const un = listen<BoxMenuActionMsg>("box-menu-action", (e) => {
      // 只处理发给自己的：Tauri 的 JS listen() 默认是 `Any` 目标，而 emit_to 的
      // 定向过滤对 `Any` 监听器无效 —— 不判这一下，每个分区窗口都会执行同一操作
      // （"改一个分区的图标大小，所有分区一起变"就是这个原因）。
      if (e.payload.boxId !== boxId) return;
      const a = e.payload.action;
      switch (a.kind) {
        case "create":
          doCreateBox(a.boxType);
          break;
        case "newFolder":
          newLinkedFolder();
          break;
        case "linkFolder":
          pickLinkFolder();
          break;
        case "iconSize":
          updateCfg({ iconSize: a.value });
          break;
        case "viewMode":
          updateCfg({ viewMode: a.value });
          break;
        case "bg":
          applyBg(a.color, a.opacity);
          break;
        case "labelColor":
          applyLabelColor(a.value);
          break;
        case "sort":
          applySort(a.value);
          break;
        case "gap":
          updateCfg({ gapX: a.gapX, gapY: a.gapY });
          break;
        // 锁定 / 解锁也放到右键菜单里：分区被锁住时既不能拖动、也不能改尺寸，
        // 一旦标题栏被裁掉（分区被拉得很窄时锁定按钮会挤出可视区），
        // 标题栏里那个开关就点不到了 —— 菜单是**独立置顶窗口**，永远不会被裁，
        // 于是这里成了不会陷入死局的兜底入口。
        case "locked":
          updateCfg({ locked: a.value });
          break;
        // 收起 / 展开（胶囊模式）。注意悬停临时展开期间配置已是"已收起"，
        // 此时再点"收起"不能走 setBoxCollapsed —— 后端幂等直接返回、窗口不会缩，
        // 得由前端把窗口缩回胶囊尺寸（doHoverCollapse）。
        case "collapse":
          if (tempExpanded) {
            doHoverCollapse();
          } else {
            setBoxCollapsed(boxId, true)
              .then(() => load())
              .catch((e) => console.error("收起分区失败", e));
          }
          break;
        case "expand":
          setBoxCollapsed(boxId, false)
            .then(() => load())
            .catch((e) => console.error("展开分区失败", e));
          break;
        case "capsuleHover":
          updateCfg({ capsuleHover: a.value });
          break;
        case "radialStyle":
          // 饼型 ⇄ 方形：只改样式落盘，弹出状态由下次单击自然生效。
          // 若此刻正弹着，先收回圆钮再由用户重新单击，避免旧几何装不下新布局
          //（pie 圆环和 grid 网格尺寸不同）。
          updateCfg({ radialStyle: a.value });
          if (tempExpanded) {
            radialSavedRect.current = null;
            collapseRadial();
          }
          break;
        case "radialShowTitle":
          updateCfg({ radialShowTitle: a.value });
          break;
        case "radialShowNames":
          updateCfg({ radialShowNames: a.value });
          break;
        case "radialIcon":
          // 本分区自己的圆钮图标（空串 = 清除，回退全局默认）
          updateCfg({ radialIcon: a.value });
          setRadialPref((p) => ({ ...p, icon: a.value }));
          break;
        case "openSettings":
          // 复用托盘菜单的"设置"动作：唤起主窗口/设置页
          invoke("tray_menu_action", { action: "settings" }).catch(() => {});
          break;
        case "absorb":
          openAbsorb();
          break;
        case "organize":
          doOrganize();
          break;
        case "delete":
          // 直接解散：不再弹确认框、不再让用户选文件怎么处理
          doDelete();
          break;
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [
    doCreateBox,
    newLinkedFolder,
    pickLinkFolder,
    updateCfg,
    applyBg,
    applyLabelColor,
    applySort,
    openAbsorb,
    doOrganize,
    doDelete,
    tempExpanded,
    doHoverCollapse,
    collapseRadial,
    load,
  ]);

  // 标题栏自适应：窄→收起项数，更窄→连搜索框一起收起。
  // 目的只有一个：排序 / 锁定 / ⋮ 三个按钮永远留在可视区内（锁定分区靠它解套）。
  useEffect(() => {
    const el = rootRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      const w = el.clientWidth;
      const next = w < 280 ? 2 : w < 330 ? 1 : 0;
      setHeadTight((prev) => (prev === next ? prev : next));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // ---------- 饼型菜单分区（radial）渲染 ----------

  // 展开态：中心钮 + 一圈气泡/卡片（pie/grid 绕主圆、rect 沿矩形四边）。
  // 点背景或中心钮收回，气泡点击打开文件。
  if (isRadial && tempExpanded) {
    const style: "pie" | "grid" | "rect" | "line" =
      cfg.radialStyle === "grid" ||
      cfg.radialStyle === "rect" ||
      cfg.radialStyle === "line"
        ? cfg.radialStyle
        : "pie";
    const showNames = cfg.radialShowNames !== false;
    // 展开布局：圆钮钉在原地不动，文件按屏幕空余自动适配（居中/平移/扇形）
    const lay = computeRadialLayout(style, items.length, showNames);
    const itemEl = (it: IconItem, i: number) =>
      style === "grid" ? (
        // 旧「方形卡片绕圆」样式（菜单已不再提供，仅老数据兼容渲染）
        <button
          key={it.path}
          className={`radial-card ${showNames ? "" : "radial-card--iconly"}`}
          style={{ "--i": i } as React.CSSProperties}
          title={displayFileName(it.name)}
          onPointerDown={(e) => beginFileDrag(e, it.path)}
          onClick={() => openItem(it)}
          onContextMenu={(e) => onItemMenu(e, it)}
        >
          {it.icon ? (
            <img className="radial-card-icon" src={it.icon} alt="" draggable={false} />
          ) : (
            <span className="radial-card-icon radial-noicon">{it.isDir ? "▣" : "▤"}</span>
          )}
          {showNames && <span className="radial-card-name">{displayFileName(it.name)}</span>}
        </button>
      ) : (
        // 圆形/矩形围绕：文件外观统一是气泡，区别只在围绕路径
        <button
          key={it.path}
          className={`radial-bubble ${showNames ? "" : "radial-bubble--iconly"}`}
          style={{ "--i": i } as React.CSSProperties}
          title={displayFileName(it.name)}
          // 按住拖动超阈值 → 系统原生 OLE 拖拽（可拖到桌面/别的分区/资源管理器）
          onPointerDown={(e) => beginFileDrag(e, it.path)}
          onClick={() => openItem(it)}
          onContextMenu={(e) => onItemMenu(e, it)}
        >
          {it.icon ? (
            <img className="radial-bubble-icon" src={it.icon} alt="" draggable={false} />
          ) : (
            <span className="radial-bubble-icon radial-noicon">{it.isDir ? "▣" : "▤"}</span>
          )}
          {showNames && <span className="radial-bubble-name">{displayFileName(it.name)}</span>}
        </button>
      );
    return (
      <div
        className={`radial-stage radial-stage--${style} ${
          radialClosing ? "radial-closing" : ""
        } ${dragOver ? "drag-over" : ""}`}
        style={{
          "--radial-item-size": `${radialPref.itemSize}px`,
          "--radial-btn-size": `${radialPref.size}px`,
          // 绽放/收拢的缩放原点钉在圆钮上：即使环心被屏幕边缘推偏，动效也从圆钮漾出
          transformOrigin: `${lay.btn[0]}px ${lay.btn[1]}px`,
        } as React.CSSProperties}
        onPointerDown={(e) => {
          // 点空白处立即收回（气泡/中心钮上的按下会 stopPropagation）
          if (e.target === e.currentTarget) collapseRadial();
        }}
      >
        {lay.pts.map(([x, y], i) => {
          const it = items[i];
          if (!it) return null;
          return (
            <div
              key={it.path}
              className="radial-orbit"
              style={{ left: lay.btn[0] + x, top: lay.btn[1] + y }}
            >
              {/* anchor 把气泡/卡片的中心（而非左上角）钉在布局点上，
                  保证所有文件中心到布局点距离一致 */}
              <div className="radial-anchor">{itemEl(it, i)}</div>
            </div>
          );
        })}
        <button
          className={`radial-center ${radialPref.icon ? "radial-center--icon" : ""} ${
            radialPref.size >= 72 ? "radial-btn--lg" : ""
          }`}
          style={{
            left: lay.btn[0],
            top: lay.btn[1],
            background: radialPref.icon ? "transparent" : box?.color || "#15b6c9",
          }}
          title={`${radialPref.title.trim() || box?.name || "分区"}（点击收回，右键菜单）`}
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => collapseRadial()}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
            openBoxMenu(boxId).catch(() => {});
          }}
        >
          {radialPref.icon ? (
            <img className="radial-center-icon" src={radialPref.icon} alt="" draggable={false} />
          ) : (
            (radialPref.title.trim() || box?.name || "分").slice(0, 2)
          )}
        </button>
      </div>
    );
  }

  // 圆钮态：radial 分区平时就是一颗小圆钮，单击弹出（不响应悬停）
  if (isRadial && cfg.collapsed) {
    const btnStyle = {
      ...bgStyle,
      "--box-bg": hexToRgba(bg.color, Math.max(bgAlpha, 0.75)),
    } as React.CSSProperties;
    // 标题与图标来自设置页：标题空 = 用分区名；图标优先于文字展示
    const btnTitle = radialPref.title.trim() || box?.name || "分区";
    return (
      <div
        className={`radial-btn ${radialPref.icon ? "radial-btn--icon" : ""} ${
          radialPref.size >= 72 ? "radial-btn--lg" : ""
        } ${dragOver ? "drag-over" : ""} ${btnHover ? "show-count" : ""}`}
        style={{ ...btnStyle, "--radial-btn-size": `${radialPref.size}px` } as React.CSSProperties}
        onPointerEnter={() => setBtnHover(true)}
        onPointerLeave={() => setBtnHover(false)}
        onPointerDown={(e) => {
          // 只记录左键：右键走 onContextMenu 弹菜单，不能再触发"轻点展开"
          if (e.button === 0) capsuleDown.current = { x: e.screenX, y: e.screenY };
          onHeaderDown(e);
        }}
        onPointerMove={onHeaderMove}
        onPointerUp={(e) => {
          // moved 要在 onHeaderUp 清掉 dragState 前读
          const moved = dragState.current?.moved === true;
          onHeaderUp(e);
          const p = capsuleDown.current;
          capsuleDown.current = null;
          // 拖动结束（位移 >2px）不弹出：先问落点是不是某个普通分区 ——
          // 是就嵌进去（圆钮消失，宿主列表里多一个条目），不是就按普通拖动落位。
          // 后端判定 = 光标点在宿主窗口矩形内，分区附近擦过不会误嵌套
          if (moved) {
            findNestTarget(boxId)
              .then((t) => {
                if (t) {
                  nestRadialBox(boxId, t.id)
                    .then(() => showToast(`已嵌套进「${t.name}」`, false))
                    .catch((err) => showToast(errText(err), true));
                } else {
                  startBoxDrag(boxId, true).catch(() => {});
                }
              })
              .catch(() => startBoxDrag(boxId, true).catch(() => {}));
            return;
          }
          if (p && Math.abs(e.screenX - p.x) < 6 && Math.abs(e.screenY - p.y) < 6) {
            expandRadial();
          }
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          openBoxMenu(boxId).catch(() => {});
        }}
      >
        {radialPref.icon ? (
          <img className="radial-btn-icon" src={radialPref.icon} alt="" draggable={false} />
        ) : cfg.radialShowTitle === false ? null : (
          <span className="radial-btn-name">{btnTitle}</span>
        )}
        {items.length > 0 && <span className="radial-btn-count">{items.length}</span>}
      </div>
    );
  }

  // 胶囊态：只渲染一条小胶囊。点击 → 永久展开；悬停 → 自动临时展开
  // （窗口尺寸由前端调整，tempExpanded=true 后走下面的完整渲染）。
  // 菜单/后端切换收起状态时会 emit box-capsule-changed，reload() 后自然走到这里。
  // 用户把分区背景调成全透明时胶囊会"隐形"，这里给背景色兜底一个最低透明度。
  if (cfg.collapsed && !tempExpanded) {
    const capsuleStyle = {
      ...bgStyle,
      "--box-bg": hexToRgba(bg.color, Math.max(bgAlpha, 0.55)),
      "--box-bg-hover": hexToRgba(bg.color, Math.max(bgAlpha, 0.8)),
    } as React.CSSProperties;
    return (
      <div
        className={`box-capsule ${dragOver ? "drag-over" : ""}`}
        style={capsuleStyle}
        onPointerDown={(e) => {
          // 记下按下位置：pointerup 时区分"点击"和"拖动结束"（拖完不该展开）。
          // 只认左键：右键是菜单，不能顺手把"轻点展开"也触发了
          if (e.button === 0) capsuleDown.current = { x: e.screenX, y: e.screenY };
          onHeaderDown(e);
        }}
        onPointerMove={onHeaderMove}
        onPointerUp={(e) => {
          onHeaderUp(e);
          const p = capsuleDown.current;
          capsuleDown.current = null;
          if (p && Math.abs(e.screenX - p.x) < 6 && Math.abs(e.screenY - p.y) < 6) {
            expandPermanently();
          }
        }}
        onPointerEnter={scheduleHoverExpand}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          openBoxMenu(boxId).catch(() => {});
        }}
      >
        <span className="box-capsule-dot" style={{ background: box?.color || "#15b6c9" }} />
        <span className="box-capsule-title">{box?.name || "分区"}</span>
        <span className="box-capsule-count">{items.length}</span>
      </div>
    );
  }

  return (
    <div
      ref={rootRef}
      className={`box-window ${cfg.locked ? "locked" : ""} ${dragOver ? "drag-over" : ""} ${
        headTight >= 1 ? "head-tight" : ""
      } ${headTight >= 2 ? "head-tight-2" : ""}`}
      style={bgStyle}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={(e) => {
        // 鼠标划过子元素时也会冒泡出 dragleave，直接 setDragOver(false) 会让状态
        // 在 true/false 之间来回抖 —— 每抖一次整个图标列表就要重渲染一遍。
        // 用 relatedTarget 判断是不是真的离开了分区。
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragOver(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
      }}
    >
      {/* 头部：拖动区 + 右键设置入口（下方网格区不再弹这个自定义菜单） */}
      <div
        className="box-win-head"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          // 菜单是独立置顶窗口：分区再小也不会被裁掉，也不会被沉到桌面层的分区挡住
          openBoxMenu(boxId).catch(() => {});
        }}
      >
        <span className="box-win-dot" style={{ background: box?.color || "#15b6c9" }} />
        {editing ? (
          <input
            ref={editRef}
            className="box-win-edit"
            value={editName}
            onChange={(e) => setEditName(e.target.value)}
            onBlur={doRename}
            onKeyDown={(e) => {
              if (e.key === "Enter") doRename();
              if (e.key === "Escape") setEditing(false);
            }}
          />
        ) : (
          <span
            className="box-win-title"
            title="双击重命名分区"
            onPointerDown={onTitleDown}
            onDoubleClick={(e) => {
              e.stopPropagation();
              setEditName(box?.name || "");
              setEditing(true);
            }}
          >
            {box?.name || "分区"}
          </span>
        )}
        {box?.boxType === "folder" && (
          <input
            className="box-win-search"
            type="text"
            placeholder="搜索…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setSearch("");
            }}
            // 别让点击输入框触发标题栏拖动 / 右键菜单；
            // 窗口点击不激活（WS_EX_NOACTIVATE），打字前要显式申请键盘焦点
            onPointerDown={(e) => {
              e.stopPropagation();
              requestKeyboardFocus();
            }}
            onContextMenu={(e) => e.stopPropagation()}
          />
        )}
        <span className="box-win-count">{filteredItems.length} 项</span>
        <button
          className={`box-win-lockbtn ${box?.sortDesc ? "on" : ""}`}
          title={
            box?.sortDesc
              ? "已按降序排列，点击切换为升序"
              : "已按升序排列，点击切换为降序"
          }
          aria-label="切换升序/降序"
          onClick={(e) => {
            e.stopPropagation();
            toggleSortDir();
          }}
        >
          {box?.sortDesc ? (
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round">
              <path d="M4 5h14M4 12h9M4 19h4" />
            </svg>
          ) : (
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round">
              <path d="M4 5h4M4 12h9M4 19h14" />
            </svg>
          )}
        </button>
        <button
          className={`box-win-lockbtn ${cfg.locked ? "locked" : ""}`}
          title={cfg.locked ? "解锁分区" : "锁定分区（不可拖动/调整大小）"}
          aria-label={cfg.locked ? "解锁分区" : "锁定分区"}
          onClick={(e) => {
            e.stopPropagation();
            updateCfg({ locked: !cfg.locked });
          }}
        >
          {cfg.locked ? (
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="4.5" y="10.5" width="15" height="10" rx="2" />
              <path d="M8 10.5V7a4 4 0 0 1 8 0v3.5" />
            </svg>
          ) : (
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
              <rect x="4.5" y="10.5" width="15" height="10" rx="2" />
              <path d="M8 10.5V7a4 4 0 0 1 7.4-2.5" />
            </svg>
          )}
        </button>
        <button
          className="box-win-more"
          title="分区菜单（新建/外观/链接/排序/解散…）"
          onClick={(e) => {
            e.stopPropagation();
            openBoxMenu(boxId).catch(() => {});
          }}
        >
          ⋮
        </button>
      </div>

      {/* 文件区：图标网格 / 列表 两种显示方式 */}
      {isList ? (
        <div className="box-win-list" onContextMenu={onBackgroundMenu}>
          {filteredItems.map((it, i) => (
            <div
              key={it.path}
              // 供"拖回本分区调整位置"定位落点
              data-idx={i}
              className="box-win-row"
              title={
                it.nestedBoxId
                  ? `${displayFileName(it.name)}（饼型菜单 · 单击展开，右键移出）`
                  : displayFileName(it.name)
              }
              onPointerDown={(e) => beginFileDrag(e, it.path)}
              // 嵌套条目单击即展开（不是文件，没有"双击打开"的概念）
              onClick={it.nestedBoxId ? (e) => openNestedEntry(e, it) : undefined}
              onDoubleClick={() => openItem(it)}
              // 文件右击 = 系统原生菜单（含我们补的「重命名」）；嵌套条目 = 移出菜单
              onContextMenu={(e) => onItemMenu(e, it)}
            >
              {it.nestedBoxId ? (
                // 嵌套的饼型分区：画一个迷你圆钮（分区色圆底 + 全局饼型图标）
                <span
                  className="box-win-nested-dot"
                  style={{ background: it.color || "#0067c0", width: 22, height: 22 }}
                >
                  {it.icon && <img src={it.icon} alt="" draggable={false} />}
                </span>
              ) : it.icon ? (
                <img
                  className="box-win-row-icon"
                  src={it.icon}
                  alt=""
                  draggable={false}
                  // 同网格视图：滚到可见范围才去提取
                  loading="lazy"
                  decoding="async"
                  onLoad={onIconLoad}
                  onError={onIconError}
                />
              ) : (
                <span className="box-win-row-icon box-win-noicon">
                  {it.isDir ? "▣" : "▤"}
                </span>
              )}
              {renaming?.path === it.path ? (
                renderRenameInput(it, "box-win-row-rename")
              ) : (
                <span
                  className="box-win-row-name"
                  style={{ color: cfg.labelColor === "#000000" ? "#000000" : "#ffffff" }}
                >
                  {displayFileName(it.name)}
                </span>
              )}
            </div>
          ))}
          {filteredItems.length === 0 && (
            <div className="box-win-empty">
              {items.length === 0 ? emptyText : "无匹配结果"}
            </div>
          )}
        </div>
      ) : (
        <div
          className="box-win-grid"
          onContextMenu={onBackgroundMenu}
          // auto-fill + 固定单元宽度：列数随宽度自动增减，图标间距恒定
          //（既不挤在一起，也不会被 1fr 拉开距离）；min(…, 0) 保证窄分区里不会横向溢出
          style={{
            gridTemplateColumns: `repeat(auto-fill, minmax(0, ${cellW}px))`,
            columnGap: cfg.gapX ?? 6,
            rowGap: cfg.gapY ?? 6,
          }}
        >
          {filteredItems.map((it, i) => (
            <div
              key={it.path}
              // 供"拖回本分区调整位置"定位落点
              data-idx={i}
              className={`box-win-item ${it.nestedBoxId ? "box-win-item-nested" : ""}`}
              title={
                it.nestedBoxId
                  ? `${displayFileName(it.name)}（饼型菜单 · 单击展开，右键移出）`
                  : displayFileName(it.name)
              }
              onPointerDown={(e) => beginFileDrag(e, it.path)}
              // 嵌套条目单击即展开（不是文件，没有"双击打开"的概念）
              onClick={it.nestedBoxId ? (e) => openNestedEntry(e, it) : undefined}
              onDoubleClick={() => openItem(it)}
              // 文件右击 = 系统原生菜单（含我们补的「重命名」）；嵌套条目 = 移出菜单
              onContextMenu={(e) => onItemMenu(e, it)}
            >
              {it.nestedBoxId ? (
                // 嵌套的饼型分区：画一个迷你圆钮（分区色圆底 + 全局饼型图标）
                <span
                  className="box-win-nested-dot"
                  style={{ background: it.color || "#0067c0", width: cfg.iconSize, height: cfg.iconSize }}
                >
                  {it.icon && <img src={it.icon} alt="" draggable={false} />}
                </span>
              ) : it.icon ? (
                <span
                  className="box-win-thumb"
                  style={{ width: cfg.iconSize, height: cfg.iconSize }}
                >
                  <img
                    src={it.icon}
                    alt=""
                    draggable={false}
                    // 按需加载：只有滚进可见范围的图标才会真的去后端提取缩略图。
                    // 几百张照片的文件夹首屏因此只提取十来张 —— 这是"秒开"的关键。
                    loading="lazy"
                    decoding="async"
                    onLoad={onIconLoad}
                    onError={onIconError}
                  />
                </span>
              ) : (
                <span className="box-win-noicon" style={{ fontSize: cfg.iconSize * 0.7 }}>
                  {it.isDir ? "▣" : "▤"}
                </span>
              )}
              {renaming?.path === it.path ? (
                renderRenameInput(it, "box-win-grid-rename")
              ) : (
                <span
                  className="box-win-label"
                  style={{
                    fontSize: Math.min(13, Math.max(10, cfg.iconSize / 5)),
                    color: cfg.labelColor === "#000000" ? "#000000" : "#ffffff",
                  }}
                >
                  {displayFileName(it.name)}
                </span>
              )}
            </div>
          ))}
          {filteredItems.length === 0 && (
            <div className="box-win-empty" style={{ gridColumn: "1 / -1" }}>
              {items.length === 0 ? emptyText : "无匹配结果"}
            </div>
          )}
        </div>
      )}

      {/* 右下角调整手柄 */}
      {!cfg.locked && (
        <div
          className="box-win-resize"
          onPointerDown={onResizeDown}
          onPointerMove={onResizeMove}
          onPointerUp={onResizeUp}
        />
      )}

      {/* 搬运结果提示（拖文件/文件夹进本分区后） */}
      {toast && (
        <div className={`box-win-toast ${toast.bad ? "bad" : "ok"}`} title={toast.text}>
          {toast.text}
        </div>
      )}

      {/* 吸附同类文件设置 */}
      {absorbOpen && (
        <div
          className="box-win-mask"
          onPointerDown={() => {
            if (!absorbBusy) setAbsorbOpen(false);
          }}
        >
          <div className="box-win-absorb" onPointerDown={(e) => e.stopPropagation()}>
            <div className="box-win-confirm-title">吸附同类文件</div>
            <div className="box-win-confirm-desc">
              勾选要收进「{box?.name}」的文件类型，桌面上已有的会立即整理进来。
            </div>
            <div className="absorb-chips">
              {EXT_GROUPS.map((g) => {
                const st = extGroupState(g.exts);
                return (
                  <button
                    key={g.key}
                    type="button"
                    className={`absorb-chip ${st}`}
                    title={g.exts.join(" ")}
                    onClick={() => toggleExtGroup(g.exts)}
                  >
                    {st === "all" ? "✓ " : st === "some" ? "◐ " : ""}
                    {g.label}
                  </button>
                );
              })}
            </div>
            <label className="absorb-field">
              <span>其他扩展名（逗号分隔）</span>
              <input
                value={absorbCustom}
                placeholder="如 psd, ai, sketch"
                onChange={(e) => setAbsorbCustom(e.target.value)}
              />
            </label>
            <label className="absorb-auto">
              <input
                type="checkbox"
                checked={absorbAuto}
                onChange={(e) => setAbsorbAuto(e.target.checked)}
              />
              <span>自动吸附新出现的同类文件</span>
            </label>
            <div className="absorb-hint">
              已选 {absorbExts.length} 种扩展名
              {absorbAuto ? "，开启自动吸附" : "，仅本次整理"}
            </div>
            {absorbMsg && <div className="absorb-msg">{absorbMsg}</div>}
            <div className="box-win-confirm-btns">
              <button
                className="absorb-primary"
                disabled={absorbBusy || absorbExts.length === 0}
                onClick={() => {
                  doAbsorb();
                }}
              >
                {absorbBusy ? "整理中…" : "开始吸附"}
              </button>
              <button disabled={absorbBusy} onClick={() => setAbsorbOpen(false)}>
                取消
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 嵌套饼型分区条目的右键小菜单（移出到桌面） */}
      {nestedMenu && (
        <div
          className="box-win-nested-menu"
          style={{ left: nestedMenu.x, top: nestedMenu.y }}
          onPointerDown={(e) => e.stopPropagation()}
          onContextMenu={(e) => e.preventDefault()}
        >
          <button
            className="box-win-nested-menu-item"
            title={`把「${nestedMenu.name}」放回桌面，恢复成独立圆钮`}
            onClick={() => unnestEntry(nestedMenu.id)}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="M12 3v12" />
              <path d="m7 8 5-5 5 5" />
              <path d="M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
            </svg>
            移出到桌面
          </button>
        </div>
      )}

      {/* 解散不再有确认浮层：点「解散分区」直接执行「文件按原路退回」 */}
    </div>
  );
}
