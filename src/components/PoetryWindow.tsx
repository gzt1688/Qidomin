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
  getPoetryWindowCfg,
  savePoetryWindowCfg,
  setPoetryEnabled,
  startPoetryDrag,
} from "../api";
import { POETRY, randomPoetry, searchPoetry, type PoetryCat, type PoetryItem } from "../poetryData";
import type { PoetryWindowCfg } from "../types";
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
  if (!m) return `rgba(255,255,255,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

const DEFAULT_CFG: PoetryWindowCfg = {
  x: 340,
  y: 180,
  w: 320,
  h: 420,
  locked: false,
  visible: true,
  bgColor: "#fbf7ee",
  bgOpacity: 94,
  autoRotate: false,
  rotateSec: 20,
};

const ROTATE_OPTIONS = [10, 20, 30, 60];

export default function PoetryWindow() {
  const [cfg, setCfg] = useState<PoetryWindowCfg>(DEFAULT_CFG);
  const [current, setCurrent] = useState<PoetryItem>(() => randomPoetry());
  const [query, setQuery] = useState("");
  const [cat, setCat] = useState<"all" | PoetryCat>("all");
  const [copied, setCopied] = useState(false);

  const cfgRef = useRef(cfg);
  cfgRef.current = cfg;

  const updateCfg = useCallback((p: Partial<PoetryWindowCfg>) => {
    // 副作用放在 updater 外面：严格模式下 updater 会被调用两次，
    // 写在里面等于一次操作发两次落盘 IPC
    const next = { ...cfgRef.current, ...p };
    cfgRef.current = next;
    setCfg(next);
    savePoetryWindowCfg(next).catch(() => {});
  }, []);

  // 首次加载
  useEffect(() => {
    getPoetryWindowCfg()
      .then(setCfg)
      .catch(() => {});
  }, []);

  // 自动轮播（鼠标悬停在组件上时暂停：用户可能正在读/学这一首）。
  // 悬停信号直接用"标题栏亮起"的同一来源：hover 时标题栏背景从透明变白
  // （.box-window:hover .box-win-head），读它的实时计算样式即可，
  // 透明置顶窗口上 pointerleave/:hover 匹配都不可靠，这个最直观也最准。
  useEffect(() => {
    if (!cfg.autoRotate) return;
    const secs = Math.max(5, cfg.rotateSec || 20);
    const headLit = () => {
      const head = document.querySelector<HTMLElement>(".poetry-window .box-win-head");
      if (!head) return false;
      const m = /rgba?\(([^)]+)\)/.exec(getComputedStyle(head).backgroundColor);
      if (!m) return false;
      const parts = m[1].split(",").map((s) => parseFloat(s));
      const alpha = parts.length >= 4 ? parts[3] : 1;
      return alpha > 0.01; // 背景不是全透明 = 标题亮着 = 鼠标悬停中
    };
    const id = window.setInterval(() => {
      if (headLit()) return; // 悬停中：这一轮跳过，相当于暂停
      setCurrent((prev) => randomPoetry(prev.id));
    }, secs * 1000);
    return () => window.clearInterval(id);
  }, [cfg.autoRotate, cfg.rotateSec]);

  const shuffle = useCallback(() => {
    setCurrent((prev) => randomPoetry(prev.id));
  }, []);

  // 复制当前显示的诗词到剪贴板（优先用 Clipboard API，失败时退回 execCommand）
  const copyCurrent = useCallback(async () => {
    const parts = [current.title + (current.year ? `（${current.year}）` : "")];
    parts.push(...current.lines);
    if (current.note) parts.push("", "注：" + current.note);
    const text = parts.join("\n");
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
    () => searchPoetry(query, cat),
    [query, cat]
  );

  const selectFromList = useCallback((it: PoetryItem) => {
    setCurrent(it);
    setQuery("");
  }, []);

  // ---------- 拖动（挂在整个窗口上，用 window 级监听，避免指针捕获/标题栏事件问题） ----------
  const onContainerDown = useCallback(
    (e: RPointerEvent) => {
      if (cfg.locked) return;
      const t = e.target as HTMLElement;
      // 输入框、按钮、列表项、菜单、缩放角等可交互元素不触发拖动
      if (t.closest("button, input, .poetry-list-item, .box-win-menu, .box-win-resize, .color-input, .menu-chip, .poetry-cat")) return;
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
        fetchSnapQuietly("poetry"),
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
      // pointermove 在高刷屏上每秒可达上百次，用 rAF 合并成一帧一次 IPC
      const flush = () => {
        raf = 0;
        if (!st) return;
        let nx = Math.round(st.wx + (lastX - st.sx) * st.scale);
        let ny = Math.round(st.wy + (lastY - st.sy) * st.scale);
        // 拖动实时磁性吸附：靠近其它分区/组件/屏幕边缘就"粘住"
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
          flush(); // 补上最后一帧，否则松手位置会差一点
        }
        // 只有真的拖动过才吸附：否则单击面板也会让它自己"跳"到旁边的分区上
        if (st && moved) startPoetryDrag(true).catch(() => {});
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
        fetchSnapQuietly("poetry"),
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
        let nw = Math.max(240, Math.round(st.w + (lastX - st.sx) * st.scale));
        let nh = Math.max(180, Math.round(st.h + (lastY - st.sy) * st.scale));
        // 拉伸吸附：右/下边缘贴齐其它分区/组件/屏幕边缘
        [nw, nh] = resizeSnap(st.px, st.py, nw, nh, st.targets, st.threshold);
        nw = Math.max(240, nw);
        nh = Math.max(180, nh);
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
    setPoetryEnabled(false).catch(() => {});
  }, []);

  const alpha = Math.min(1, Math.max(0, cfg.bgOpacity / 100));
  const bgStyle = {
    "--box-bg": hexToRgba(cfg.bgColor, alpha),
    "--box-bg-hover": hexToRgba(cfg.bgColor, Math.min(1, alpha + 0.08)),
  } as CSSProperties;

  // ---------- 右键菜单：发布给独立菜单窗口渲染（不再画在组件里被裁掉） ----------
  const menuModel = useCallback(
    (): MenuItem[] => [
      { t: "action", id: "close", label: "关闭组件", danger: true },
      { t: "action", id: "shuffle", label: "换一首（随机）" },
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
    publishMenu("poetry", menuModel());
  }, [menuModel]);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    // 依赖变化太快时 listen 的订阅可能晚于清理完成——这时要立刻退订，
    // 否则旧监听器永远退不掉、越积越多（一次点击被当成 N 次切换）
    let disposed = false;
    void onMenuRequest("poetry", () => publishMenu("poetry", menuModel())).then(
      (f) => {
        if (disposed) f();
        else off1 = f;
      }
    );
    void onMenuPick("poetry", (id, value) => {
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
  }, [
    menuModel,
    hideWindow,
    shuffle,
    updateCfg,
    cfg.locked,
    cfg.bgColor,
    cfg.bgOpacity,
  ]);

  const searching = query.trim().length > 0;

  return (
    <div
      className={`box-window poetry-window ${cfg.locked ? "locked" : ""}`}
      style={bgStyle}
      onPointerDown={onContainerDown}
      onContextMenu={(e) => {
        e.preventDefault();
        // 菜单是独立置顶窗口：组件再矮也裁不到它
        openWidgetMenu("poetry").catch(() => {});
      }}
    >
      <div
        className="box-win-head"
      >
        <span className="box-win-dot" style={{ background: "#b8632e" }} />
        <span className="box-win-title">毛泽东诗词</span>
        <span className="box-win-count">{POETRY.length} 首</span>
        <button
          className={`box-win-btn ${cfg.autoRotate ? "rotate-on" : "rotate-off"}`}
          title={
            cfg.autoRotate
              ? `自动轮播：开启中（每 ${cfg.rotateSec}s 换一首，右键菜单可调间隔；点击关闭）`
              : "自动轮播：已关闭（点击开启）"
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
            openWidgetMenu("poetry").catch(() => {});
          }}
        >
          ⋮
        </button>
      </div>

      {/* 搜索 + 分类 */}
      <div className="poetry-search">
        <input
          className="poetry-input"
          value={query}
          placeholder="搜索标题 / 内容 / 名句…"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setQuery("");
          }}
        />
        <div className="poetry-cats">
          {(["all", "poem", "quote"] as const).map((k) => (
            <button
              key={k}
              className={`poetry-cat ${cat === k ? "on" : ""}`}
              onClick={() => setCat(k)}
            >
              {k === "all" ? "全部" : k === "poem" ? "诗词" : "名言"}
            </button>
          ))}
        </div>
      </div>

      {/* 内容区 */}
      <div className="poetry-body">
        {searching ? (
          results.length === 0 ? (
            <div className="poetry-empty">没有匹配的「{query}」</div>
          ) : (
            <div className="poetry-list">
              {results.map((it) => (
                <button key={it.id} className="poetry-list-item" onClick={() => selectFromList(it)}>
                  <span className={`poetry-cattag ${it.cat}`}>{it.cat === "poem" ? "诗词" : "名言"}</span>
                  <span className="poetry-list-title">{it.title}</span>
                  <span className="poetry-list-sub">{it.lines[0]}</span>
                </button>
              ))}
            </div>
          )
        ) : (
          <div className="poetry-detail">
            <div className="poetry-detail-head">
              <span className={`poetry-cattag ${current.cat}`}>{current.cat === "poem" ? "诗词" : "名言"}</span>
              <span className="poetry-title">{current.title}</span>
              {current.year && <span className="poetry-year">{current.year}</span>}
            </div>
            <div className="poetry-lines">
              {current.lines.map((ln, i) => (
                <p key={i}>{ln}</p>
              ))}
            </div>
            {current.note && <div className="poetry-note">{current.note}</div>}
            <div className="poetry-detail-actions">
              <button className="poetry-next" onClick={shuffle}>
                换一首
              </button>
              <button className="poetry-copy" onClick={copyCurrent}>
                {copied ? "已复制" : "复制"}
              </button>
            </div>
          </div>
        )}
      </div>

      {!cfg.locked && (
        <div
          className="box-win-resize"
          onPointerDown={onResizeDown}
        />
      )}

    </div>
  );
}
