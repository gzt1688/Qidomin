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
  getShanghanWindowCfg,
  saveShanghanWindowCfg,
  setShanghanEnabled,
  startShanghanDrag,
} from "../api";
import {
  loadShanghan,
  randomChapter,
  searchShanghan,
  neighbor,
  firstOf,
  chapterKey,
  type ShanghanChapter,
  type ShanghanData,
} from "../shanghanData";
import type { ShanghanWindowCfg } from "../types";
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
  if (!m) return `rgba(240,244,245,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

const DEFAULT_CFG: ShanghanWindowCfg = {
  x: 820,
  y: 200,
  w: 360,
  h: 460,
  locked: false,
  visible: true,
  bgColor: "#eaf0f2",
  bgOpacity: 95,
  autoRotate: false,
  rotateSec: 30,
};

const ROTATE_OPTIONS = [15, 30, 60, 120];

export default function ShanghanWindow() {
  const [cfg, setCfg] = useState<ShanghanWindowCfg>(DEFAULT_CFG);
  const [data, setData] = useState<ShanghanData | null>(null);
  const [current, setCurrent] = useState<ShanghanChapter | null>(null);
  const [query, setQuery] = useState("");
  const [copied, setCopied] = useState(false);

  const cfgRef = useRef(cfg);
  cfgRef.current = cfg;

  const updateCfg = useCallback((p: Partial<ShanghanWindowCfg>) => {
    const next = { ...cfgRef.current, ...p };
    cfgRef.current = next;
    setCfg(next);
    saveShanghanWindowCfg(next).catch(() => {});
  }, []);

  // 首次加载：配置 + 数据 + 随机一条
  useEffect(() => {
    getShanghanWindowCfg()
      .then(setCfg)
      .catch(() => {});
    loadShanghan()
      .then((d) => {
        setData(d);
        return randomChapter();
      })
      .then((c) => setCurrent(c))
      .catch((e) => console.error(e));
  }, []);

  // 自动轮播（随机展示条文）：鼠标悬停时暂停（读标题栏亮起状态，同诗词组件）
  useEffect(() => {
    if (!cfg.autoRotate || !data) return;
    const secs = Math.max(5, cfg.rotateSec || 30);
    const headLit = () => {
      const head = document.querySelector<HTMLElement>(".shanghan-window .box-win-head");
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
    const text = `《伤寒论》${current.title}\n\n${current.text}`;
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
    () => searchShanghan(query).catch(() => []),
    [query]
  );
  const [hitList, setHitList] = useState<ShanghanChapter[]>([]);
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

  const selectFromList = useCallback((it: ShanghanChapter) => {
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

  // ---------- 拖动（同诗词组件：window 级监听） ----------
  const onContainerDown = useCallback(
    (e: RPointerEvent) => {
      if (cfg.locked) return;
      const t = e.target as HTMLElement;
      if (t.closest("button, input, .sh-list-item, .box-win-menu, .box-win-resize, .color-input, .menu-chip")) return;
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
        fetchSnapQuietly("shanghan"),
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
        if (st && moved) startShanghanDrag(true).catch(() => {});
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
        fetchSnapQuietly("shanghan"),
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
    setShanghanEnabled(false).catch(() => {});
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
      { t: "action", id: "shuffle", label: "换一条（随机）" },
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
    publishMenu("shanghan", menuModel());
  }, [menuModel]);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    let disposed = false;
    void onMenuRequest("shanghan", () => publishMenu("shanghan", menuModel())).then(
      (f) => {
        if (disposed) f();
        else off1 = f;
      }
    );
    void onMenuPick("shanghan", (id, value) => {
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
    ? `共 ${data.meta.chapters} 条`
    : "加载中…";

  const isSearching = query.trim().length > 0;

  return (
    <div
      className={`box-window shanghan-window ${cfg.locked ? "locked" : ""}`}
      style={bgStyle}
      onPointerDown={onContainerDown}
      onContextMenu={(e) => {
        e.preventDefault();
        openWidgetMenu("shanghan").catch(() => {});
      }}
    >
      <div className="box-win-head">
        <span className="box-win-dot" style={{ background: "#2f7d8c" }} />
        <span className="box-win-title">伤寒论</span>
        <span className="box-win-count">{totalText}</span>
        <button
          className={`box-win-btn ${cfg.autoRotate ? "rotate-on" : "rotate-off"}`}
          title={
            cfg.autoRotate
              ? `自动轮播：开启中（每 ${cfg.rotateSec}s 换一条，右键菜单可调间隔；点击关闭）`
              : "自动轮播：已关闭（点击开启，随机展示条文）"
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
            openWidgetMenu("shanghan").catch(() => {});
          }}
        >
          ⋮
        </button>
      </div>

      {/* 搜索 */}
      <div className="sh-search">
        <input
          className="sh-input"
          value={query}
          placeholder="搜索条文 / 方证（如：桂枝汤、少阳、脉浮）…"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setQuery("");
          }}
        />
      </div>

      {/* 内容区 */}
      <div className="sh-body">
        {!data ? (
          <div className="sh-empty">正在加载《伤寒论》…</div>
        ) : isSearching ? (
          searching ? (
            <div className="sh-empty">搜索中…</div>
          ) : hitList.length === 0 ? (
            <div className="sh-empty">没有匹配「{query}」的条文</div>
          ) : (
            <div className="sh-list">
              {hitList.map((it) => (
                <button key={chapterKey(it)} className="sh-list-item" onClick={() => selectFromList(it)}>
                  <span className="sh-list-title">{it.title}</span>
                  <span className="sh-list-sub">{it.text.slice(0, 24).replace(/\n/g, " ")}…</span>
                </button>
              ))}
            </div>
          )
        ) : current ? (
          <div className="sh-detail">
            <div className="sh-detail-head">
              <span className="sh-booktag">伤寒论</span>
              <span className="sh-title">{current.title}</span>
            </div>
            <div className="sh-lines">
              {current.text.split("\n").map((ln, i) => (
                <p key={i}>{ln}</p>
              ))}
            </div>
            <div className="sh-detail-actions">
              <button className="sh-nav" onClick={() => step(-1)} disabled={!current}>
                上一条
              </button>
              <button className="sh-next" onClick={shuffle}>
                换一条
              </button>
              <button className="sh-nav" onClick={() => step(1)} disabled={!current}>
                下一条
              </button>
              <button className="sh-copy" onClick={copyCurrent}>
                {copied ? "已复制" : "复制"}
              </button>
            </div>
          </div>
        ) : (
          <div className="sh-empty">无内容</div>
        )}
      </div>

      {!cfg.locked && (
        <div className="box-win-resize" onPointerDown={onResizeDown} />
      )}
    </div>
  );
}
