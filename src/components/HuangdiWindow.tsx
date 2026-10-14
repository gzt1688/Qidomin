import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as RPointerEvent,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import {
  getHuangdiWindowCfg,
  saveHuangdiWindowCfg,
  setHuangdiEnabled,
  startHuangdiDrag,
} from "../api";
import {
  loadHuangdi,
  randomChapter,
  searchHuangdi,
  neighbor,
  firstOf,
  chapterKey,
  BOOK_ORDER,
  type HuangdiBook,
  type HuangdiChapter,
  type HuangdiData,
} from "../huangdiData";
import type { HuangdiWindowCfg } from "../types";
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
  if (!m) return `rgba(245,245,240,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

const DEFAULT_CFG: HuangdiWindowCfg = {
  x: 380,
  y: 200,
  w: 360,
  h: 460,
  locked: false,
  visible: true,
  bgColor: "#f4efe3",
  bgOpacity: 95,
  autoRotate: false,
  rotateSec: 30,
};

const ROTATE_OPTIONS = [15, 30, 60, 120];

export default function HuangdiWindow() {
  const [cfg, setCfg] = useState<HuangdiWindowCfg>(DEFAULT_CFG);
  const [data, setData] = useState<HuangdiData | null>(null);
  const [current, setCurrent] = useState<HuangdiChapter | null>(null);
  const [query, setQuery] = useState("");
  const [book, setBook] = useState<"" | HuangdiBook>("");
  const [copied, setCopied] = useState(false);

  const cfgRef = useRef(cfg);
  cfgRef.current = cfg;

  const updateCfg = useCallback((p: Partial<HuangdiWindowCfg>) => {
    const next = { ...cfgRef.current, ...p };
    cfgRef.current = next;
    setCfg(next);
    saveHuangdiWindowCfg(next).catch(() => {});
  }, []);

  // 首次加载：配置 + 数据 + 随机一篇
  useEffect(() => {
    getHuangdiWindowCfg()
      .then(setCfg)
      .catch(() => {});
    loadHuangdi()
      .then((d) => {
        setData(d);
        return randomChapter();
      })
      .then((c) => setCurrent(c))
      .catch((e) => console.error(e));
  }, []);

  // 自动轮播（随机展示经文）：鼠标悬停时暂停（读标题栏亮起状态，同诗词组件）
  useEffect(() => {
    if (!cfg.autoRotate || !data) return;
    const secs = Math.max(5, cfg.rotateSec || 30);
    const headLit = () => {
      const head = document.querySelector<HTMLElement>(".huangdi-window .box-win-head");
      if (!head) return false;
      const m = /rgba?\(([^)]+)\)/.exec(getComputedStyle(head).backgroundColor);
      if (!m) return false;
      const parts = m[1].split(",").map((s) => parseFloat(s));
      const alpha = parts.length >= 4 ? parts[3] : 1;
      return alpha > 0.01;
    };
    const id = window.setInterval(() => {
      if (headLit()) return;
      setCurrent((prev) => {
        void randomChapter(prev ? chapterKey(prev) : undefined).then((c) => {
          if (c) setCurrent(c);
        });
        return prev;
      });
    }, secs * 1000);
    return () => window.clearInterval(id);
  }, [cfg.autoRotate, cfg.rotateSec, data]);

  const shuffle = useCallback(() => {
    setCurrent((prev) => {
      void randomChapter(prev ? chapterKey(prev) : undefined).then((c) => {
        if (c) setCurrent(c);
      });
      return prev;
    });
  }, []);

  const copyCurrent = useCallback(async () => {
    if (!current) return;
    const text = `《黄帝内经·${current.book}》${current.title}\n\n${current.text}`;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
      } else {
        throw new Error("no-clipboard");
      }
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.focus();
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        /* 忽略 */
      }
      document.body.removeChild(ta);
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1200);
  }, [current]);

  const results = useMemo(
    () => searchHuangdi(query, book).catch(() => []),
    [query, book]
  );
  const [hitList, setHitList] = useState<HuangdiChapter[]>([]);
  const [searching, setSearching] = useState(false);
  useEffect(() => {
    if (!query.trim()) {
      setHitList([]);
      setSearching(false);
      return;
    }
    let alive = true;
    setSearching(true);
    void results.then((list) => {
      if (alive) {
        setHitList(list);
        setSearching(false);
      }
    });
    return () => {
      alive = false;
    };
  }, [results, query]);

  const selectFromList = useCallback((it: HuangdiChapter) => {
    setCurrent(it);
    setQuery("");
  }, []);

  const step = useCallback(
    (dir: -1 | 1) => {
      if (!current) return;
      void neighbor(current, dir).then((c) => {
        if (c) setCurrent(c);
      });
    },
    [current]
  );

  const jumpBook = useCallback(
    (b: HuangdiBook) => {
      void firstOf(b).then((c) => {
        if (c) setCurrent(c);
      });
    },
    []
  );

  // ---------- 拖动（同诗词组件：window 级监听） ----------
  const onContainerDown = useCallback(
    (e: RPointerEvent) => {
      if (cfg.locked) return;
      const t = e.target as HTMLElement;
      if (t.closest("button, input, .hd-list-item, .box-win-menu, .box-win-resize, .color-input, .menu-chip, .hd-book")) return;
      if (e.button !== 0) return;
      e.preventDefault();
      const win = getCurrentWindow();
      win.setFocus().catch(() => {});
      let st: {
        sx: number; sy: number; wx: number; wy: number; scale: number;
        ww: number; wh: number;
        targets: [number, number, number, number][];
        threshold: number;
      } | null = null;
      let moved = false;
      let raf = 0;
      let lastX = e.screenX;
      let lastY = e.screenY;
      Promise.all([
        win.outerPosition(),
        win.outerSize(),
        win.scaleFactor().catch(() => 1),
        fetchSnapQuietly("huangdi"),
      ])
        .then(([pos, sz, scale, snap]) => {
          st = {
            sx: e.screenX, sy: e.screenY, wx: pos.x, wy: pos.y,
            ww: sz.width, wh: sz.height,
            targets: snap.rects, threshold: snap.threshold,
            scale: typeof scale === "number" && scale > 0 ? scale : 1,
          };
        })
        .catch(() => {});
      const flush = () => {
        raf = 0;
        if (!st) return;
        let nx = Math.round(st.wx + (lastX - st.sx) * st.scale);
        let ny = Math.round(st.wy + (lastY - st.sy) * st.scale);
        [nx, ny] = magnetSnap(nx, ny, st.ww, st.wh, st.targets, st.threshold);
        getCurrentWindow().setPosition(new PhysicalPosition(nx, ny)).catch(() => {});
      };
      const onMove = (ev: PointerEvent) => {
        if (!st) return;
        if (Math.abs(ev.screenX - st.sx) > 2 || Math.abs(ev.screenY - st.sy) > 2) moved = true;
        lastX = ev.screenX;
        lastY = ev.screenY;
        if (!raf) raf = requestAnimationFrame(flush);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        if (raf) {
          cancelAnimationFrame(raf);
          raf = 0;
          flush();
        }
        if (st && moved) startHuangdiDrag(true).catch(() => {});
        st = null;
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [cfg.locked]
  );

  // ---------- 调整大小 ----------
  const onResizeDown = useCallback(
    (e: RPointerEvent) => {
      if (cfg.locked) return;
      e.stopPropagation();
      e.preventDefault();
      const win = getCurrentWindow();
      let st: {
        sx: number; sy: number; w: number; h: number; scale: number;
        px: number; py: number;
        targets: [number, number, number, number][];
        threshold: number;
      } | null = null;
      Promise.all([
        win.outerSize(),
        win.scaleFactor().catch(() => 1),
        win.outerPosition(),
        fetchSnapQuietly("huangdi"),
      ])
        .then(([sz, scale, pos, snap]) => {
          st = {
            sx: e.screenX, sy: e.screenY, w: sz.width, h: sz.height,
            px: pos.x, py: pos.y,
            targets: snap.rects, threshold: snap.threshold,
            scale: typeof scale === "number" && scale > 0 ? scale : 1,
          };
        })
        .catch(() => {});
      let raf = 0;
      let lastX = e.screenX;
      let lastY = e.screenY;
      const flush = () => {
        raf = 0;
        if (!st) return;
        let nw = Math.max(280, Math.round(st.w + (lastX - st.sx) * st.scale));
        let nh = Math.max(200, Math.round(st.h + (lastY - st.sy) * st.scale));
        [nw, nh] = resizeSnap(st.px, st.py, nw, nh, st.targets, st.threshold);
        nw = Math.max(280, nw);
        nh = Math.max(200, nh);
        getCurrentWindow().setSize(new PhysicalSize(nw, nh)).catch(() => {});
      };
      const onMove = (ev: PointerEvent) => {
        if (!st) return;
        lastX = ev.screenX;
        lastY = ev.screenY;
        if (!raf) raf = requestAnimationFrame(flush);
      };
      const onUp = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        if (raf) {
          cancelAnimationFrame(raf);
          raf = 0;
          flush();
        }
        st = null;
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [cfg.locked]
  );

  const hideWindow = useCallback(() => {
    setHuangdiEnabled(false).catch(() => {});
  }, []);

  const alpha = Math.min(1, Math.max(0, cfg.bgOpacity / 100));
  const bgStyle = {
    "--box-bg": hexToRgba(cfg.bgColor, alpha),
    "--box-bg-hover": hexToRgba(cfg.bgColor, Math.min(1, alpha + 0.08)),
  } as CSSProperties;

  // ---------- 右键菜单 ----------
  const menuModel = useCallback(
    (): MenuItem[] => [
      { t: "action", id: "close", label: "关闭组件", danger: true },
      { t: "action", id: "shuffle", label: "换一篇（随机）" },
      { t: "label", text: "自动轮播（开关在标题栏）" },
      {
        t: "chips",
        id: "rotateSec",
        value: String(cfg.rotateSec),
        options: ROTATE_OPTIONS.map((s) => ({ value: String(s), label: `${s}s` })),
      },
      { t: "label", text: "背景色 / 透明度" },
      { t: "color", id: "bg", color: cfg.bgColor, opacity: cfg.bgOpacity },
      {
        t: "action",
        id: "lock",
        label: cfg.locked ? "解锁面板" : "锁定面板",
      },
      { t: "action", id: "hide", label: "隐藏面板", danger: true },
    ],
    [cfg.rotateSec, cfg.bgColor, cfg.bgOpacity, cfg.locked]
  );

  useEffect(() => {
    publishMenu("huangdi", menuModel());
  }, [menuModel]);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    let disposed = false;
    void onMenuRequest("huangdi", () => publishMenu("huangdi", menuModel())).then(
      (f) => {
        if (disposed) f();
        else off1 = f;
      }
    );
    void onMenuPick("huangdi", (id, value) => {
      switch (id) {
        case "close":
        case "hide":
          hideWindow();
          break;
        case "shuffle":
          shuffle();
          break;
        case "rotateSec":
          updateCfg({ rotateSec: Number(value), autoRotate: true });
          break;
        case "lock":
          updateCfg({ locked: !cfgRef.current.locked });
          break;
        case "bg:color":
          updateCfg({ bgColor: String(value ?? cfgRef.current.bgColor) });
          break;
        case "bg:opacity":
          updateCfg({ bgOpacity: Number(value ?? cfgRef.current.bgOpacity) });
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
  }, [menuModel, hideWindow, shuffle, updateCfg, cfg.locked, cfg.bgColor, cfg.bgOpacity]);

  const totalText = data
    ? `共 ${data.meta.total_chapters} 篇`
    : "加载中…";

  const isSearching = query.trim().length > 0;

  return (
    <div
      className={`box-window huangdi-window ${cfg.locked ? "locked" : ""}`}
      style={bgStyle}
      onPointerDown={onContainerDown}
      onContextMenu={(e) => {
        e.preventDefault();
        openWidgetMenu("huangdi").catch(() => {});
      }}
    >
      <div className="box-win-head">
        <span className="box-win-dot" style={{ background: "#8a6d3b" }} />
        <span className="box-win-title">黄帝内经</span>
        <span className="box-win-count">{totalText}</span>
        <button
          className={`box-win-btn ${cfg.autoRotate ? "rotate-on" : "rotate-off"}`}
          title={
            cfg.autoRotate
              ? `自动轮播：开启中（每 ${cfg.rotateSec}s 换一篇，右键菜单可调间隔；点击关闭）`
              : "自动轮播：已关闭（点击开启，随机展示经文）"
          }
          onClick={(e) => {
            e.stopPropagation();
            updateCfg({ autoRotate: !cfgRef.current.autoRotate });
          }}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
            <path d="m17 2 4 4-4 4" />
            <path d="M3 11v-1a4 4 0 0 1 4-4h14" />
            <path d="m7 22-4-4 4-4" />
            <path d="M21 13v1a4 4 0 0 1-4 4H3" />
          </svg>
        </button>
        <button
          className={`box-win-lockbtn ${cfg.locked ? "locked" : ""}`}
          title={cfg.locked ? "解锁面板" : "锁定面板（不可拖动/调整大小）"}
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
          title="面板菜单"
          onClick={(e) => {
            e.stopPropagation();
            openWidgetMenu("huangdi").catch(() => {});
          }}
        >
          ⋮
        </button>
      </div>

      {/* 搜索 + 书筛选 */}
      <div className="hd-search">
        <input
          className="hd-input"
          value={query}
          placeholder="搜索篇名 / 经文（如：上古天真、阴阳、五行）…"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setQuery("");
          }}
        />
        <div className="hd-books">
          <button
            className={`hd-book ${book === "" ? "on" : ""}`}
            onClick={() => setBook("")}
          >
            全部
          </button>
          {BOOK_ORDER.map((b) => (
            <button
              key={b}
              className={`hd-book ${book === b ? "on" : ""}`}
              onClick={() => setBook(b)}
            >
              {b}
            </button>
          ))}
        </div>
      </div>

      {/* 内容区 */}
      <div className="hd-body">
        {!data ? (
          <div className="hd-empty">正在加载《黄帝内经》…</div>
        ) : isSearching ? (
          searching ? (
            <div className="hd-empty">搜索中…</div>
          ) : hitList.length === 0 ? (
            <div className="hd-empty">没有匹配「{query}」的篇章</div>
          ) : (
            <div className="hd-list">
              {hitList.map((it) => (
                <button key={chapterKey(it)} className="hd-list-item" onClick={() => selectFromList(it)}>
                  <span className={`hd-booktag ${it.book}`}>{it.book}</span>
                  <span className="hd-list-title">{it.title}</span>
                  <span className="hd-list-sub">{it.text.slice(0, 24).replace(/\n/g, " ")}…</span>
                </button>
              ))}
            </div>
          )
        ) : current ? (
          <div className="hd-detail">
            <div className="hd-detail-head">
              <span className={`hd-booktag ${current.book}`}>{current.book}</span>
              <span className="hd-title">{current.title}</span>
            </div>
            <div className="hd-lines">
              {current.text.split("\n").map((ln, i) => (
                <p key={i}>{ln}</p>
              ))}
            </div>
            <div className="hd-detail-actions">
              <button className="hd-nav" onClick={() => step(-1)} disabled={!current}>
                上一篇
              </button>
              <button className="hd-next" onClick={shuffle}>
                换一篇
              </button>
              <button className="hd-nav" onClick={() => step(1)} disabled={!current}>
                下一篇
              </button>
              <button className="hd-copy" onClick={copyCurrent}>
                {copied ? "已复制" : "复制"}
              </button>
            </div>
          </div>
        ) : (
          <div className="hd-empty">无内容</div>
        )}
      </div>

      {!cfg.locked && (
        <div className="box-win-resize" onPointerDown={onResizeDown} />
      )}
    </div>
  );
}
