import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as RPointerEvent,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { listen } from "@tauri-apps/api/event";
import {
  addNote,
  addNoteTab,
  deleteNote,
  deleteNoteTab,
  getNoteWindowCfg,
  getNotes,
  listNoteTabs,
  renameNoteTab,
  saveNote,
  saveNoteWindowCfg,
  setNotesEnabled,
  startNoteDrag,
} from "../api";
import type { Note, NoteTab, NoteWindowCfg } from "../types";
import { applyCalc, calcTail } from "../calc";
import { fetchSnapQuietly, magnetSnap, resizeSnap } from "../snap";
import {
  onMenuPick,
  onMenuRequest,
  openWidgetMenu,
  publishMenu,
  type MenuItem,
} from "../widgetMenu";

function hexToRgba(hex: string, alpha: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec((hex || "").trim());
  if (!m) return `rgba(255,246,204,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** 本地时间 → 数据库存的时间格式 "YYYY-MM-DD HH:MM" */
function fmtDateTime(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ${pad2(
    d.getHours()
  )}:${pad2(d.getMinutes())}`;
}

/** "YYYY-MM-DD HH:MM" → datetime-local 输入框需要的 "YYYY-MM-DDTHH:MM" */
function toInputValue(s: string): string {
  return s.replace(" ", "T").slice(0, 16);
}

/** 紧凑显示：09-15 09:00 */
function shortTime(s: string): string {
  const [d, t] = s.split(" ");
  return `${(d || "").slice(5)} ${t || ""}`;
}

/** 修改时间紧凑显示：今年只写 09-15 09:00，往年补上年份；空值不显示 */
function shortModified(s: string): string {
  const v = (s || "").trim();
  if (!v) return "";
  const [d, hm] = v.split(" ");
  const p = (d || "").split("-");
  if (p.length < 3) return v.slice(0, 16);
  const md = `${p[1]}-${p[2]}`;
  const time = (hm || "").slice(0, 5);
  const sameYear = p[0] === String(new Date().getFullYear());
  return `${sameYear ? md : `${p[0]}-${md}`} ${time}`.trim();
}

/**
 * 点击正文时把屏幕坐标换算成文本偏移。
 * 编辑态用的是 textarea，非编辑态是同样排版的一层文字 —— 点进去要让光标落在
 * 用户点的那个字旁边，而不是每次都跳到末尾。点空白处 / 末尾时间则落到结尾。
 */
function caretOffsetFromPoint(x: number, y: number, view: HTMLElement, max: number): number {
  const doc = document as Document & {
    caretPositionFromPoint?: (x: number, y: number) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  let node: Node | null = null;
  let offset = 0;
  if (typeof doc.caretPositionFromPoint === "function") {
    const p = doc.caretPositionFromPoint(x, y);
    if (p) {
      node = p.offsetNode;
      offset = p.offset;
    }
  } else if (typeof doc.caretRangeFromPoint === "function") {
    const r = doc.caretRangeFromPoint(x, y);
    if (r) {
      node = r.startContainer;
      offset = r.startOffset;
    }
  }
  const textEl = view.querySelector(".note-text-static");
  if (!node || node.nodeType !== Node.TEXT_NODE || !textEl || !textEl.contains(node)) {
    return max;
  }
  return Math.min(Math.max(0, offset), max);
}

const POP_W = 236;
const POP_H = 116;

export default function NotesWindow() {
  const [notes, setNotes] = useState<Note[]>([]);
  const [cfg, setCfg] = useState<NoteWindowCfg>({
    x: 120,
    y: 120,
    w: 380,
    h: 320,
    locked: false,
    visible: true,
    bgColor: "#fff6cc",
    bgOpacity: 94,
    calc: true,
  });
  const [bg, setBg] = useState({ color: "#fff6cc", opacity: 94 });
  /** 正在编辑提醒的那一行 */
  const [editId, setEditId] = useState<number | null>(null);
  const [pop, setPop] = useState<{ left: number; top: number } | null>(null);
  const [draft, setDraft] = useState("");
  const [alarm, setAlarm] = useState(true);
  /** 到点提醒横幅 */
  const [banner, setBanner] = useState<Note | null>(null);

  // ---------- 便签页（标题栏标签）----------
  /** 所有便签页 */
  const [tabs, setTabs] = useState<NoteTab[]>([]);
  /** 当前页 id */
  const [activeTab, setActiveTab] = useState(0);
  /** 正在就地改名的那一页标签 */
  const [renamingTab, setRenamingTab] = useState<{ id: number; name: string } | null>(null);
  const tabInputRef = useRef<HTMLInputElement | null>(null);
  /** 回调里要读"当前页"，用 ref 传，免得 load / ensureTail 跟着重建 */
  const activeTabRef = useRef(0);
  const renamingRef = useRef<{ id: number; name: string } | null>(null);

  const notesRef = useRef<Note[]>([]);
  const timers = useRef<Record<number, number>>({});
  /** 防止并发补空行 */
  const tailing = useRef(false);

  /** 正在编辑（展开成输入框）的那一行；null = 全部按纯文本显示 */
  const [editingId, setEditingId] = useState<number | null>(null);
  const editingIdRef = useRef<number | null>(null);
  /** 进入编辑态时光标该落的位置（null = 末尾） */
  const pendingCaret = useRef<number | null>(null);
  useEffect(() => {
    editingIdRef.current = editingId;
  }, [editingId]);

  const applyNotes = useCallback((list: Note[]) => {
    notesRef.current = list;
    setNotes(list);
  }, []);

  // ---------- 保存（防抖，避免每敲一个字都写库） ----------
  const persist = useCallback((id: number) => {
    const n = notesRef.current.find((x) => x.id === id);
    if (n) saveNote(n).catch(() => {});
  }, []);

  const flushAll = useCallback(() => {
    Object.values(timers.current).forEach((t) => window.clearTimeout(t));
    timers.current = {};
    notesRef.current.forEach((n) => saveNote(n).catch(() => {}));
  }, []);

  const patch = useCallback(
    (id: number, p: Partial<Note>, immediate = false) => {
      applyNotes(
        notesRef.current.map((n) => {
          if (n.id !== id) return n;
          const next = { ...n, ...p };
          // 只有正文真的变了才刷新"最后修改时间"：失焦、设提醒这些动作不算改动
          if (p.content !== undefined && p.content !== n.content) {
            next.modifiedAt = fmtDateTime(new Date());
          }
          return next;
        })
      );
      window.clearTimeout(timers.current[id]);
      if (immediate) {
        persist(id);
      } else {
        timers.current[id] = window.setTimeout(() => persist(id), 400);
      }
    },
    [applyNotes, persist]
  );

  /** 末尾始终保留一行空白：直接点上去就能写，不需要"新增"按钮 */
  const ensureTail = useCallback(async () => {
    const list = notesRef.current;
    const last = list[list.length - 1];
    if (last && last.content.trim() === "") return;
    if (tailing.current) return;
    tailing.current = true;
    try {
      // 空行补到"当前这一页"
      const tabId = activeTabRef.current;
      const id = await addNote(tabId);
      // 只追加这一条，不做整表重载，避免覆盖用户正在输入、尚未落库的内容
      applyNotes([
        ...notesRef.current,
        {
          id,
          content: "",
          remindAt: null,
          remindAlarm: true,
          fired: false,
          tabId,
          modifiedAt: fmtDateTime(new Date()),
        },
      ]);
    } catch (e) {
      console.error("新增空行失败", e);
    } finally {
      tailing.current = false;
    }
  }, [applyNotes]);

  /** 加载某一页的便签（顺手把窗口配置也带回来，省一次 IPC 来回） */
  const load = useCallback(async (tabId: number) => {
    try {
      const [raw, c] = await Promise.all([getNotes(tabId), getNoteWindowCfg()]);
      // 清理夹在中间的空便签（末尾空行保留）
      const keep: Note[] = [];
      raw.forEach((n, i) => {
        const isEmpty = n.content.trim() === "" && !n.remindAt;
        if (isEmpty && i !== raw.length - 1) {
          deleteNote(n.id).catch(() => {});
          return;
        }
        keep.push(n);
      });
      applyNotes(keep);
      // 老版本存下的配置里没有 calc 字段 → 缺省视为开启
      setCfg({ ...c, calc: c.calc !== false });
      setBg({ color: c.bgColor || "#fff6cc", opacity: c.bgOpacity ?? 94 });
      await ensureTail();
    } catch (e) {
      console.error("加载便签失败", e);
    }
  }, [applyNotes, ensureTail]);

  useEffect(() => {
    (async () => {
      try {
        // 先取标签列表，再加载第一页
        const list = await listNoteTabs();
        const first = list[0]?.id ?? 0;
        setTabs(list);
        activeTabRef.current = first;
        setActiveTab(first);
        await load(first);
      } catch (e) {
        console.error("加载便签失败", e);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 窗口失焦 / 隐藏前把没落库的内容写下去
  useEffect(() => {
    const onBlur = () => flushAll();
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [flushAll]);

  // 到点提醒：后端推来的事件
  useEffect(() => {
    const un = listen<Note>("note-remind", (e) => {
      flushAll();
      setBanner(e.payload);
      // 提醒可能来自别的一页 → 切过去，让用户直接看到是哪一条
      const tabId = e.payload.tabId;
      if (tabId && tabId !== activeTabRef.current) {
        activeTabRef.current = tabId;
        setActiveTab(tabId);
        load(tabId);
        return;
      }
      getNotes(activeTabRef.current)
        .then((list) => {
          applyNotes(list);
          ensureTail();
        })
        .catch(() => {});
    });
    return () => {
      un.then((f) => f());
    };
  }, [applyNotes, ensureTail, flushAll, load]);

  // ---------- 便签页操作 ----------

  /** 切换页：先把当前页没落库的内容写下去，再整页换掉 */
  const switchTab = useCallback(
    async (tabId: number) => {
      if (tabId === activeTabRef.current) return;
      flushAll();
      setBanner(null);
      setRenamingTab(null);
      renamingRef.current = null;
      activeTabRef.current = tabId;
      setActiveTab(tabId);
      await load(tabId);
    },
    [flushAll, load]
  );

  /** 新建一页，建完直接进改名 */
  const addTab = useCallback(async () => {
    try {
      flushAll();
      const id = await addNoteTab("");
      const list = await listNoteTabs();
      setTabs(list);
      activeTabRef.current = id;
      setActiveTab(id);
      applyNotes([]);
      await load(id);
      const r = { id, name: list.find((t) => t.id === id)?.name ?? "便签" };
      renamingRef.current = r;
      setRenamingTab(r);
    } catch (e) {
      console.error("新建便签失败", e);
    }
  }, [applyNotes, flushAll, load]);

  const startTabRename = useCallback((t: NoteTab) => {
    const r = { id: t.id, name: t.name };
    renamingRef.current = r;
    setRenamingTab(r);
  }, []);

  const endTabRename = useCallback(() => {
    renamingRef.current = null;
    setRenamingTab(null);
  }, []);

  /** 标签改名：回车 / 失焦提交
   *  （提交和取消都先走 endTabRename，保证只执行一次） */
  const commitTabRename = useCallback(() => {
    const r = renamingRef.current;
    endTabRename();
    if (!r) return;
    const name = r.name.trim();
    if (!name) return;
    renameNoteTab(r.id, name)
      .then(() => listNoteTabs())
      .then(setTabs)
      .catch((e) => console.error("便签改名失败", e));
  }, [endTabRename]);

  /** 删除一页（连同里面的内容）；后端保证至少留一页 */
  const removeTab = useCallback(
    async (tabId: number) => {
      const t = tabs.find((x) => x.id === tabId);
      const label = t?.name ?? "这一页";
      if (!window.confirm(`删除便签「${label}」？里面的内容会一起删掉。`)) return;
      try {
        flushAll();
        await deleteNoteTab(tabId);
        const list = await listNoteTabs();
        setTabs(list);
        const next = list[0]?.id ?? 0;
        activeTabRef.current = next;
        setActiveTab(next);
        applyNotes([]);
        await load(next);
      } catch (e) {
        console.error("删除便签失败", e);
      }
    },
    [applyNotes, flushAll, load, tabs]
  );

  // 改名输入框出现时聚焦并全选
  useEffect(() => {
    const el = tabInputRef.current;
    if (!el || !renamingTab) return;
    el.focus();
    el.select();
    // 只在换了另一页标签时定位，输入过程中不要重置选区
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renamingTab?.id]);

  // cfg 的 ref 镜像：updateCfg 要在 setState updater 之外读到最新值
  const cfgRef = useRef(cfg);
  useEffect(() => {
    cfgRef.current = cfg;
  }, [cfg]);

  // bg 的 ref 镜像：菜单 pick 回调里读最新背景色/透明度，不用闭包旧值
  const bgRef = useRef(bg);
  useEffect(() => {
    bgRef.current = bg;
  }, [bg]);

  const updateCfg = useCallback((p: Partial<NoteWindowCfg>) => {
    // 副作用放在 updater 外面：严格模式下 updater 会被调用两次，
    // 写在里面等于一次操作发两次落盘 IPC
    const next = { ...cfgRef.current, ...p };
    cfgRef.current = next;
    setCfg(next);
    saveNoteWindowCfg(next).catch(() => {});
  }, []);

  const applyBg = useCallback(
    (color: string, opacity: number) => {
      setBg({ color, opacity });
      updateCfg({ bgColor: color, bgOpacity: opacity });
    },
    [updateCfg]
  );

  // ---------- 行操作 ----------
  /**
   * 输入一行。
   *
   * 末尾等号 = 一个"算一下"的指令：`6+3=` 当场补成 `6+3=9`，
   * 结果插在等号正后方、光标自然落到末尾，可以接着往下算
   * （`6+3=9+1=` → `6+3=9+1=10`）。判定与求值见 `calc.ts`。
   *
   * 两个不动手的前提：
   * - `composing`：输入法组字过程中的中间态不是用户想留下的内容；
   * - **只在内容变长时算**（`value.length > prev.length`）。否则算完按下退格删掉结果，
   *   又会立刻被补回来 —— 结果那一位就永远删不掉了。
   */
  const onType = useCallback(
    (n: Note, value: string, composing = false) => {
      let next = value;
      if (!composing && cfgRef.current.calc !== false && value.length > n.content.length) {
        const hit = calcTail(value);
        if (hit) next = applyCalc(value, hit);
      }
      patch(n.id, { content: next });
      if (next.trim() !== "") ensureTail();
    },
    [patch, ensureTail]
  );

  /** 失焦时清掉中间的空行，并补上末尾空行 */
  const onRowBlur = useCallback(
    (n: Note) => {
      const list = notesRef.current;
      const isLast = list[list.length - 1]?.id === n.id;
      const cur = list.find((x) => x.id === n.id);
      if (cur && cur.content.trim() === "" && !cur.remindAt && !isLast) {
        window.clearTimeout(timers.current[n.id]);
        delete timers.current[n.id];
        deleteNote(n.id).catch(() => {});
        applyNotes(list.filter((x) => x.id !== n.id));
      } else {
        patch(n.id, {}, true);
      }
      ensureTail();
      // 只有还停在这一行时才收起编辑态（切到别的行时 editingId 已经是新行了）
      setEditingId((cur2) => (cur2 === n.id ? null : cur2));
    },
    [applyNotes, patch, ensureTail]
  );

  /**
   * 进入某一行的编辑态。
   * caret = 数字 → 光标落到该偏移；caret = null → 不动光标（别打断浏览器自己放好的位置）
   */
  const beginEdit = useCallback((id: number, caret: number | null) => {
    pendingCaret.current = caret;
    setEditingId(id);
  }, []);

  /** 输入框自己拿到焦点（比如点末尾空行）：只标记编辑态，光标归浏览器管 */
  const onEditorFocus = useCallback((id: number) => {
    // 同 onRowMouseDown：确保窗口真的在前台，键盘事件才进得来
    getCurrentWindow().setFocus().catch(() => {});
    pendingCaret.current = null;
    setEditingId(id);
  }, []);

  /** 编辑态切进来之后：聚焦输入框并把光标放到该放的位置 */
  useEffect(() => {
    if (editingId == null) return;
    const el = document.querySelector<HTMLTextAreaElement>(`textarea[data-id="${editingId}"]`);
    if (!el) return;
    const caret = pendingCaret.current;
    pendingCaret.current = null;
    // 关键：组件窗口沉在桌面层，点击时不一定被系统激活。DOM 焦点给了输入框
    // （光标会闪），但 Win32 键盘焦点可能还留在别的窗口上 —— 表现为光标闪、
    // 按键全没反应。显式把窗口拉回前台，保证键盘事件真的进得来。
    getCurrentWindow().setFocus().catch(() => {});
    el.focus();
    if (caret == null) return; // 光标已经是对的，别多手去 setSelectionRange
    const pos = Math.min(Math.max(0, caret), el.value.length);
    try {
      el.setSelectionRange(pos, pos);
    } catch {
      /* ignore */
    }
  }, [editingId]);

  /** 点行内任意位置（正文、序号、空白处）直接进入编辑，光标落在点的位置 */
  const onRowMouseDown = useCallback(
    (e: React.MouseEvent, n: Note) => {
      const t = e.target as HTMLElement;
      if (t.closest("button") || t.tagName === "TEXTAREA") return;
      e.preventDefault();
      // 从别的行切过来：被换掉的输入框不一定补 blur 事件，这里手动补一次，
      // 免得上一行该清的空行没清、该落盘的内容没落盘
      const prevId = editingIdRef.current;
      if (prevId != null && prevId !== n.id) {
        const prev = notesRef.current.find((x) => x.id === prevId);
        if (prev) onRowBlur(prev);
      }
      const view = t.closest(".note-view") as HTMLElement | null;
      // 点在正文上 → 光标落在点的那个字旁边；点在序号/行尾空白 → 落在末尾
      beginEdit(
        n.id,
        view ? caretOffsetFromPoint(e.clientX, e.clientY, view, n.content.length) : n.content.length
      );
    },
    [beginEdit, onRowBlur]
  );

  /** 点列表下方的空白：聚焦到末尾那行空行 */
  const focusTail = useCallback(
    (e: React.MouseEvent) => {
      if (e.target !== e.currentTarget) return;
      const last = notesRef.current[notesRef.current.length - 1];
      if (!last) return;
      e.preventDefault();
      const el = document.querySelector<HTMLTextAreaElement>(`textarea[data-id="${last.id}"]`);
      if (el) {
        el.focus();
        const len = el.value.length;
        try {
          el.setSelectionRange(len, len);
        } catch {
          /* ignore */
        }
        return;
      }
      beginEdit(last.id, last.content.length);
    },
    [beginEdit]
  );

  const removeRow = useCallback(
    (id: number) => {
      window.clearTimeout(timers.current[id]);
      delete timers.current[id];
      deleteNote(id).catch(() => {});
      applyNotes(notesRef.current.filter((n) => n.id !== id));
      if (editId === id) {
        setEditId(null);
        setPop(null);
      }
      setEditingId((cur) => (cur === id ? null : cur));
      ensureTail();
    },
    [applyNotes, editId, ensureTail]
  );

  // ---------- 提醒设置浮层 ----------
  const openRemind = useCallback(
    (n: Note, e: React.MouseEvent) => {
      if (editId === n.id) {
        setEditId(null);
        setPop(null);
        return;
      }
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      const left = Math.min(
        Math.max(6, r.right - POP_W),
        Math.max(6, window.innerWidth - POP_W - 6)
      );
      let top = r.bottom + 6;
      if (top + POP_H > window.innerHeight - 6) {
        top = Math.max(6, r.top - POP_H - 6);
      }
      setAlarm(n.remindAlarm);
      setDraft(
        n.remindAt
          ? toInputValue(n.remindAt)
          : toInputValue(fmtDateTime(new Date(Date.now() + 3600_000)))
      );
      setEditId(n.id);
      setPop({ left, top });
    },
    [editId]
  );

  const closeRemind = useCallback(() => {
    setEditId(null);
    setPop(null);
  }, []);

  const confirmRemind = useCallback(() => {
    if (editId == null || !draft) return;
    patch(editId, { remindAt: draft.replace("T", " "), remindAlarm: alarm, fired: false }, true);
    closeRemind();
  }, [alarm, closeRemind, draft, editId, patch]);

  const clearRemind = useCallback(() => {
    if (editId == null) return;
    patch(editId, { remindAt: null, fired: false }, true);
    closeRemind();
  }, [closeRemind, editId, patch]);

  const snooze = useCallback(
    (n: Note, minutes: number) => {
      patch(
        n.id,
        { remindAt: fmtDateTime(new Date(Date.now() + minutes * 60_000)), fired: false },
        true
      );
      setBanner(null);
    },
    [patch]
  );

  // ---------- 拖动（手动屏幕坐标，避免系统模态拖动卡死） ----------
  const dragState = useRef<{
    sx: number;
    sy: number;
    wx: number;
    wy: number;
    /** 窗口尺寸（物理像素），磁性吸附用 */
    ww: number;
    wh: number;
    /** 磁性吸附候选 [x,y,w,h] + 阈值 */
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
    (e: RPointerEvent) => {
      if (cfg.locked || (e.target as HTMLElement).closest("button")) return;
      if (e.button !== 0) return;
      if (dragState.current?.active) return;
      const win = getCurrentWindow();
      try {
        e.currentTarget.setPointerCapture(e.pointerId);
      } catch {
        /* ignore */
      }
      e.preventDefault();
      win.setFocus().catch(() => {});
      Promise.all([
        win.outerPosition(),
        win.outerSize(),
        win.scaleFactor().catch(() => 1),
        fetchSnapQuietly("notes"),
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
    [cfg.locked]
  );

  const onHeaderMove = useCallback((e: RPointerEvent) => {
    const ds = dragState.current;
    if (!ds?.active) return;
    ds.lastX = e.screenX;
    ds.lastY = e.screenY;
    const dx = (e.screenX - ds.sx) * ds.scale;
    const dy = (e.screenY - ds.sy) * ds.scale;
    if (Math.abs(dx) > 2 || Math.abs(dy) > 2) ds.moved = true;
    if (!ds.moved) return;
    if (rafRef.current != null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      const s = dragState.current;
      if (!s?.active) return;
      let nx = s.wx + Math.round((s.lastX - s.sx) * s.scale);
      let ny = s.wy + Math.round((s.lastY - s.sy) * s.scale);
      // 拖动实时磁性吸附：靠近其它分区/组件/屏幕边缘就"粘住"，
      // 反方向拖超过阈值才挣脱（绝对坐标计算，不会累积漂移）
      [nx, ny] = magnetSnap(nx, ny, s.ww, s.wh, s.targets, s.threshold);
      getCurrentWindow()
        .setPosition(new PhysicalPosition(nx, ny))
        .catch(() => {});
    });
  }, []);

  const onHeaderUp = useCallback((e: RPointerEvent) => {
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
    if (ds.moved) startNoteDrag(true).catch(() => {});
  }, []);

  // ---------- 调整大小 ----------
  const resizeState = useRef<{
    sx: number;
    sy: number;
    w: number;
    h: number;
    px: number;
    py: number;
    targets: [number, number, number, number][];
    threshold: number;
    scale: number;
    /** 最新鼠标位置：rAF 回调里读它，避免用过期的 pointermove 事件 */
    lastX: number;
    lastY: number;
  } | null>(
    null
  );
  const resizeRaf = useRef<number | null>(null);

  const onResizeDown = useCallback(
    (e: RPointerEvent) => {
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
        fetchSnapQuietly("notes"),
      ])
        .then(([sz, scale, pos, snap]) => {
          resizeState.current = {
            sx: e.screenX,
            sy: e.screenY,
            w: sz.width,
            h: sz.height,
            px: pos.x,
            py: pos.y,
            targets: snap.rects,
            threshold: snap.threshold,
            scale: typeof scale === "number" && scale > 0 ? scale : 1,
            lastX: e.screenX,
            lastY: e.screenY,
          };
        })
        .catch(() => {});
    },
    [cfg.locked]
  );

  const onResizeMove = useCallback((e: RPointerEvent) => {
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
      let nw = Math.max(220, Math.round(s.w + (s.lastX - s.sx) * s.scale));
      let nh = Math.max(120, Math.round(s.h + (s.lastY - s.sy) * s.scale));
      // 拉伸吸附：右/下边缘在阈值内就贴到候选边上（其它分区/组件/屏幕边缘）
      [nw, nh] = resizeSnap(s.px, s.py, nw, nh, s.targets, s.threshold);
      nw = Math.max(220, nw);
      nh = Math.max(120, nh);
      getCurrentWindow()
        .setSize(new PhysicalSize(nw, nh))
        .catch(() => {});
    });
  }, []);

  const onResizeUp = useCallback((e: RPointerEvent) => {
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

  /** 内容框随文字自动增高 */
  const autoGrow = (el: HTMLTextAreaElement | null) => {
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  };

  const hideWindow = useCallback(() => {
    flushAll();
    setNotesEnabled(false).catch(() => {});
  }, [flushAll]);

  const bgAlpha = Math.min(1, Math.max(0, bg.opacity / 100));
  const bgStyle = {
    "--note-bg": hexToRgba(bg.color, bgAlpha),
    "--note-bg-solid": hexToRgba(bg.color, Math.min(1, bgAlpha + 0.06)),
  } as CSSProperties;

  // ---------- 右键菜单：发布给独立菜单窗口渲染（不再画在组件里被裁掉） ----------
  const menuModel = useCallback(
    (): MenuItem[] => [
      { t: "action", id: "close", label: "关闭组件", danger: true },
      { t: "label", text: "背景色 / 透明度" },
      { t: "color", id: "bg", color: bg.color, opacity: bg.opacity },
      { t: "label", text: "算式自动计算" },
      {
        t: "chips",
        id: "calc",
        value: cfg.calc ? "on" : "off",
        options: [
          { value: "on", label: "开" },
          { value: "off", label: "关" },
        ],
      },
      {
        t: "action",
        id: "lock",
        label: cfg.locked ? "解锁便签" : "锁定便签",
      },
      { t: "action", id: "hide", label: "隐藏便签", danger: true },
    ],
    [bg.color, bg.opacity, cfg.locked, cfg.calc]
  );

  // 状态一变就重发一次，菜单里看到的永远是最新值
  useEffect(() => {
    publishMenu("notes", menuModel());
  }, [menuModel]);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    // 依赖变化太快时 listen 的订阅可能晚于清理完成——这时要立刻退订，
    // 否则旧监听器永远退不掉、越积越多（一次点击被当成 N 次切换）
    let disposed = false;
    void onMenuRequest("notes", () => publishMenu("notes", menuModel())).then(
      (f) => {
        if (disposed) f();
        else off1 = f;
      }
    );
    void onMenuPick("notes", (id, value) => {
      switch (id) {
        case "close":
        case "hide":
          hideWindow();
          break;
        case "lock":
          updateCfg({ locked: !cfgRef.current.locked });
          break;
        case "calc":
          updateCfg({ calc: String(value) === "on" });
          break;
        case "bg:color":
          applyBg(String(value ?? bgRef.current.color), bgRef.current.opacity);
          break;
        case "bg:opacity":
          applyBg(bgRef.current.color, Number(value ?? bgRef.current.opacity));
          break;
      }
    }).then((f) => {
      if (disposed) f();
      else off2 = f;
    });
    return () => {
      disposed = true;
      off1?.();
      off2?.();
    };
  }, [menuModel, hideWindow, updateCfg, applyBg, cfg.locked, bg.color, bg.opacity]);


  /** 渲染一个便签标签（标题栏最多放 2 个，超出部分由 .note-tabs-extra 第二行承接） */
  const renderTab = (t: (typeof tabs)[number]) => (
    <div
      key={t.id}
      className={`note-tab ${t.id === activeTab ? "active" : ""}`}
      title="单击切换 · 双击改名"
      onClick={() => switchTab(t.id)}
      onDoubleClick={(e) => {
        e.stopPropagation();
        startTabRename(t);
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        if (tabs.length > 1) removeTab(t.id);
      }}
    >
      {renamingTab?.id === t.id ? (
        <input
          ref={tabInputRef}
          className="note-tab-input"
          value={renamingTab.name}
          spellCheck={false}
          onChange={(e) => {
            const next = { id: t.id, name: e.target.value };
            renamingRef.current = next;
            setRenamingTab(next);
          }}
          onClick={(e) => e.stopPropagation()}
          onDoubleClick={(e) => e.stopPropagation()}
          onBlur={commitTabRename}
          onKeyDown={(e) => {
            if (e.key === "Enter") commitTabRename();
            else if (e.key === "Escape") endTabRename();
          }}
        />
      ) : (
        <>
          <span className="note-tab-name">{t.name}</span>
          {tabs.length > 1 && t.id === activeTab && (
            <span
              className="note-tab-del"
              title="删除这个便签"
              onClick={(e) => {
                e.stopPropagation();
                removeTab(t.id);
              }}
            >
              ×
            </span>
          )}
        </>
      )}
    </div>
  );

  const editing = notes.find((n) => n.id === editId);

  return (
    <div
      className={`note-window ${cfg.locked ? "locked" : ""}`}
      style={bgStyle}
      onContextMenu={(e) => {
        e.preventDefault();
        // 菜单是独立置顶窗口：组件再矮也裁不到它，也不会被沉到桌面层的组件挡住
        openWidgetMenu("notes").catch(() => {});
      }}
    >
      <div
        className="box-win-head"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <span className="box-win-dot" style={{ background: "#e8b93b" }} />
        <span className="box-win-count">{notes.filter((n) => n.content.trim()).length} 条</span>
        <button
          className={`box-win-lockbtn ${cfg.locked ? "locked" : ""}`}
          title={cfg.locked ? "解锁便签" : "锁定便签（不可拖动/调整大小）"}
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
          title="便签菜单"
          onClick={(e) => {
            e.stopPropagation();
            openWidgetMenu("notes").catch(() => {});
          }}
        >
          ⋮
        </button>

        {/* 标签栏：一个标签 = 一个便签；单击切换、双击改名，右键或 × 删除。
            标题栏里最多放 2 个，超出部分在第二行（.note-tabs-extra）显示。 */}
        <div className="note-tabs" onPointerDown={(e) => e.stopPropagation()}>
          {tabs.slice(0, 2).map(renderTab)}
        </div>
        <button
          className="note-tab-add"
          title="新建便签"
          onClick={(e) => {
            e.stopPropagation();
            addTab();
          }}
        >
          ＋
        </button>
      </div>

      {/* 标签超过 2 个时，多出来的在标题栏下面的内容区第二行显示 */}
      {tabs.length > 2 && (
        <div className="note-tabs-extra" onPointerDown={(e) => e.stopPropagation()}>
          {tabs.slice(2).map(renderTab)}
        </div>
      )}

      {/* 到点提醒横幅 */}
      {banner && (
        <div className="note-banner">
          <div className="note-banner-title">
            {banner.remindAlarm ? "🔔 闹钟提醒" : "⏰ 便签提醒"}
          </div>
          <div className="note-banner-text">{banner.content.trim() || "（空便签）"}</div>
          <div className="note-banner-btns">
            <button className="btn small ghost" onClick={() => snooze(banner, 5)}>
              5 分钟后
            </button>
            <button className="btn small" onClick={() => setBanner(null)}>
              知道了
            </button>
          </div>
        </div>
      )}

      <div className="note-list" onMouseDown={focusTail}>
        {notes.map((n, i) => {
          const done = !!n.remindAt && n.fired;
          const empty = n.content.trim() === "";
          // 空行（含末尾那行）平时就是输入框：占位提示可见、点上去就能写；
          // 有内容的行只在编辑时展开成输入框，平时是纯文本 + 尾部修改时间
          const showEditor = editingId === n.id || empty;
          return (
            <div
              className={`note-row ${done ? "fired" : ""}`}
              key={n.id}
              onMouseDown={(e) => onRowMouseDown(e, n)}
            >
              <span className="note-no">{i + 1}</span>
              <div className="note-body">
                {showEditor ? (
                  <textarea
                    className="note-text"
                    data-id={n.id}
                    value={n.content}
                    placeholder={i === notes.length - 1 ? "点这里写点什么…" : ""}
                    rows={1}
                    ref={(el) => autoGrow(el)}
                    onChange={(e) =>
                      onType(
                        n,
                        e.target.value,
                        (e.nativeEvent as InputEvent).isComposing === true
                      )
                    }
                    onInput={(e) => autoGrow(e.currentTarget)}
                    onFocus={() => onEditorFocus(n.id)}
                    onBlur={() => onRowBlur(n)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && !e.shiftKey) {
                        e.preventDefault();
                        (e.currentTarget as HTMLTextAreaElement).blur();
                      }
                    }}
                  />
                ) : (
                  <div className="note-view" data-id={n.id}>
                    <span className="note-text-static">{n.content}</span>
                    {/* 修改时间跟着正文流走：正文多长都贴在末尾，换行了也在最后一行后面 */}
                    {n.modifiedAt ? (
                      <span className="note-time">· {shortModified(n.modifiedAt)}</span>
                    ) : null}
                  </div>
                )}
              </div>
              <button
                className={`note-bell ${n.remindAt ? "set" : ""} ${done ? "done" : ""} ${
                  empty && !n.remindAt ? "ghost" : ""
                }`}
                title={
                  n.remindAt
                    ? `${n.remindAlarm ? "有闹钟" : "无闹钟"} · ${n.remindAt}`
                    : "设置提醒"
                }
                onClick={(e) => {
                  e.stopPropagation();
                  openRemind(n, e);
                }}
              >
                {n.remindAt ? (
                  <>
                    <span className="note-bell-ico">{done ? "✓" : n.remindAlarm ? "🔔" : "⏰"}</span>
                    {shortTime(n.remindAt)}
                  </>
                ) : (
                  <span className="note-bell-ico">⏰</span>
                )}
              </button>
              <button
                className="note-del"
                title="删除这行"
                tabIndex={-1}
                onClick={(e) => {
                  e.stopPropagation();
                  removeRow(n.id);
                }}
              >
                ×
              </button>
            </div>
          );
        })}
      </div>

      {/* 提醒设置浮层（不占行高，保持列表紧凑） */}
      {editing && pop && (
        <>
          <div className="note-pop-mask" onMouseDown={closeRemind} />
          <div className="note-pop" style={{ left: pop.left, top: pop.top }}>
            <input
              className="note-dt"
              type="datetime-local"
              value={draft}
              autoFocus
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") confirmRemind();
                if (e.key === "Escape") closeRemind();
              }}
            />
            <div className="note-pop-row">
              <div className="note-seg">
                <button
                  className={`note-seg-btn ${alarm ? "on" : ""}`}
                  onClick={() => setAlarm(true)}
                >
                  有闹钟
                </button>
                <button
                  className={`note-seg-btn ${!alarm ? "on" : ""}`}
                  onClick={() => setAlarm(false)}
                >
                  无闹钟
                </button>
              </div>
              <span className="note-pop-tip">{alarm ? "到点响铃" : "静默提示"}</span>
            </div>
            <div className="note-pop-btns">
              <button className="btn small ghost" onClick={clearRemind}>
                清除
              </button>
              <button className="btn small" onClick={confirmRemind}>
                确定
              </button>
            </div>
          </div>
        </>
      )}

      {!cfg.locked && (
        <div
          className="box-win-resize"
          onPointerDown={onResizeDown}
          onPointerMove={onResizeMove}
          onPointerUp={onResizeUp}
        />
      )}

    </div>
  );
}
