import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as RPointerEvent,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import {
  addStock,
  deleteStock,
  fetchStocks,
  getStockWindowCfg,
  getStocks,
  saveStockWindowCfg,
  setStocksEnabled,
  startStockDrag,
} from "../api";
import type { StockItem, StockQuote, StockWindowCfg } from "../types";
import { fetchSnapQuietly, magnetSnap, resizeSnap } from "../snap";
import {
  onMenuPick,
  onMenuRequest,
  openWidgetMenu,
  publishMenu,
  type MenuItem,
} from "../widgetMenu";

/// A 股交易时段（周一至周五 9:30-11:30 / 13:00-15:00）。
/// 收盘、周末、节假日没必要按秒级刷新，降到 5 分钟一次。
function isTradingTime(): boolean {
  const d = new Date();
  const day = d.getDay();
  if (day === 0 || day === 6) return false;
  const m = d.getHours() * 60 + d.getMinutes();
  return (m >= 570 && m <= 690) || (m >= 780 && m <= 900);
}

function hexToRgba(hex: string, alpha: number): string {
  const m = /^#?([0-9a-f]{6})$/i.exec((hex || "").trim());
  if (!m) return `rgba(255,255,255,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

const DEFAULT_CFG: StockWindowCfg = {
  x: 200,
  y: 160,
  w: 320,
  h: 300,
  locked: false,
  visible: true,
  bgColor: "#ffffff",
  bgOpacity: 96,
  interval: 5,
};

const INTERVAL_OPTIONS = [3, 5, 10, 30, 60];

/** 涨/跌/平的配色（A 股习惯：红涨绿跌） */
function tone(v: number | null | undefined): "up" | "down" | "flat" {
  if (v == null) return "flat";
  if (v > 0) return "up";
  if (v < 0) return "down";
  return "flat";
}

function fmtPrice(v: number | null): string {
  return v == null ? "--" : v.toFixed(2);
}

function fmtPct(v: number | null): string {
  if (v == null) return "--";
  return `${v > 0 ? "+" : ""}${v.toFixed(2)}%`;
}

function hhmmss(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function hhmm(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}

export default function StockWindow() {
  const [stocks, setStocks] = useState<StockItem[]>([]);
  const [quotes, setQuotes] = useState<Record<string, StockQuote>>({});
  const [cfg, setCfg] = useState<StockWindowCfg>(DEFAULT_CFG);
  const [input, setInput] = useState("");
  /** 添加自选浮层是否展开 */
  const [addOpen, setAddOpen] = useState(false);
  /** 添加请求进行中 */
  const [busyAdd, setBusyAdd] = useState(false);
  const [err, setErr] = useState("");
  const [updatedAt, setUpdatedAt] = useState(0);
  const [spinning, setSpinning] = useState(false);

  const stocksRef = useRef<StockItem[]>([]);
  const busyRef = useRef(false);
  const errTimer = useRef<number | null>(null);

  const showErr = useCallback((msg: string) => {
    setErr(msg);
    if (errTimer.current != null) window.clearTimeout(errTimer.current);
    errTimer.current = window.setTimeout(() => setErr(""), 4500);
  }, []);

  // 面板被隐藏（组件"关闭"只是隐藏窗口）时停掉轮询：窗口不销毁、组件不卸载，
  // 光靠 document.hidden 在 WebView2 上并不可靠，这里改用后端发的可见性事件。
  const [paused, setPaused] = useState(false);
  const pausedRef = useRef(false);
  useEffect(() => {
    pausedRef.current = paused;
  }, [paused]);
  useEffect(() => {
    let un: (() => void) | undefined;
    listen<boolean>("stocks-paused", (e) => setPaused(!e.payload))
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => {
      un?.();
    };
  }, []);

  // ---------- 行情刷新 ----------
  const refresh = useCallback(async () => {
    if (document.hidden || pausedRef.current) return;
    const list = stocksRef.current;
    if (list.length === 0) {
      setQuotes({});
      setUpdatedAt(Date.now());
      return;
    }
    if (busyRef.current) return;
    busyRef.current = true;
    setSpinning(true);
    try {
      const list2 = await fetchStocks(list.map((s) => s.secid));
      const m: Record<string, StockQuote> = {};
      list2.forEach((q) => {
        m[q.secid] = q;
      });
      setQuotes(m);
      setUpdatedAt(Date.now());
      setErr("");
    } catch (e) {
      showErr(String(e));
    } finally {
      busyRef.current = false;
      setSpinning(false);
    }
  }, [showErr]);

  // 首次加载
  useEffect(() => {
    (async () => {
      try {
        const [list, c] = await Promise.all([getStocks(), getStockWindowCfg()]);
        stocksRef.current = list;
        setStocks(list);
        setCfg(c);
        refresh();
      } catch (e) {
        showErr(String(e));
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 自动刷新（间隔可调）。暂停（面板隐藏）时完全不起定时器；非交易时段降到 5 分钟。
  useEffect(() => {
    if (paused) return;
    const secs = isTradingTime() ? Math.max(2, cfg.interval || 5) : 300;
    const id = window.setInterval(() => refresh(), secs * 1000);
    refresh();
    return () => window.clearInterval(id);
  }, [cfg.interval, refresh, paused]);

  // cfg 的 ref 镜像：updateCfg 要在 setState updater 之外读到最新值并落盘
  const cfgRef = useRef(cfg);
  useEffect(() => {
    cfgRef.current = cfg;
  }, [cfg]);

  const updateCfg = useCallback((p: Partial<StockWindowCfg>) => {
    // 副作用必须放在 updater 外面：React 严格模式下 updater 会被调用两次，
    // 写在里面等于一次操作发两次落盘 IPC。
    const next = { ...cfgRef.current, ...p };
    cfgRef.current = next;
    setCfg(next);
    saveStockWindowCfg(next).catch(() => {});
  }, []);

  // ---------- 自选股增删 ----------
  const doAdd = useCallback(async () => {
    const code = input.trim();
    if (!code || busyAdd) return;
    setBusyAdd(true);
    try {
      const item = await addStock(code);
      const next = [...stocksRef.current, item];
      stocksRef.current = next;
      setStocks(next);
      setInput("");
      setAddOpen(false);
      await refresh();
    } catch (e) {
      showErr(String(e));
    } finally {
      setBusyAdd(false);
    }
  }, [busyAdd, input, refresh, showErr]);

  const doDelete = useCallback(
    async (id: number) => {
      const prev = stocksRef.current;
      const next = prev.filter((s) => s.id !== id);
      stocksRef.current = next;
      setStocks(next);
      try {
        await deleteStock(id);
      } catch (e) {
        // 失败要回滚：否则界面显示已删、后端还在，只能等下次启动才对上
        stocksRef.current = prev;
        setStocks(prev);
        showErr(String(e));
      }
    },
    [showErr]
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
        fetchSnapQuietly("stocks"),
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
    if (ds.moved) startStockDrag(true).catch(() => {});
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
        fetchSnapQuietly("stocks"),
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
      let nh = Math.max(140, Math.round(s.h + (s.lastY - s.sy) * s.scale));
      // 拉伸吸附：右/下边缘在阈值内就贴到候选边上（其它分区/组件/屏幕边缘）
      [nw, nh] = resizeSnap(s.px, s.py, nw, nh, s.targets, s.threshold);
      nw = Math.max(220, nw);
      nh = Math.max(140, nh);
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

  const hideWindow = useCallback(() => {
    setStocksEnabled(false).catch(() => {});
  }, []);

  const alpha = Math.min(1, Math.max(0, cfg.bgOpacity / 100));
  const bgStyle = {
    "--stock-bg": hexToRgba(cfg.bgColor, alpha),
    "--stock-bg-solid": hexToRgba(cfg.bgColor, Math.min(1, alpha + 0.04)),
  } as CSSProperties;

  // ---------- 右键菜单：发布给独立菜单窗口渲染（不再画在组件里被裁掉） ----------
  const menuModel = useCallback(
    (): MenuItem[] => [
      { t: "action", id: "close", label: "关闭组件", danger: true },
      { t: "action", id: "add", label: "添加自选股…" },
      { t: "label", text: "刷新间隔" },
      {
        t: "chips",
        id: "interval",
        value: String(cfg.interval),
        options: INTERVAL_OPTIONS.map((s) => ({ value: String(s), label: `${s}s` })),
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
    [cfg.interval, cfg.bgColor, cfg.bgOpacity, cfg.locked]
  );

  useEffect(() => {
    publishMenu("stock", menuModel());
  }, [menuModel]);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    // 依赖变化太快时 listen 的订阅可能晚于清理完成——这时要立刻退订，
    // 否则旧监听器永远退不掉、越积越多（一次点击被当成 N 次切换）
    let disposed = false;
    void onMenuRequest("stock", () => publishMenu("stock", menuModel())).then(
      (f) => {
        if (disposed) f();
        else off1 = f;
      }
    );
    void onMenuPick("stock", (id, value) => {
      switch (id) {
        case "close":
        case "hide":
          hideWindow();
          break;
        case "add":
          setAddOpen(true);
          break;
        case "interval":
          updateCfg({ interval: Number(value) });
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
    updateCfg,
    cfg.locked,
    cfg.bgColor,
    cfg.bgOpacity,
  ]);

  const tradingCount = stocks.length;

  return (
    <div
      className={`stock-window ${cfg.locked ? "locked" : ""}`}
      style={bgStyle}
      onContextMenu={(e) => {
        e.preventDefault();
        // 菜单是独立置顶窗口：组件再矮也裁不到它
        openWidgetMenu("stock").catch(() => {});
      }}
    >
      <div
        className="box-win-head"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <span className="box-win-dot" style={{ background: "#15b6c9" }} />
        <span className="box-win-title">股票</span>
        <span className="box-win-count" title={updatedAt ? `最后更新 ${hhmmss(updatedAt)}` : ""}>
          {tradingCount > 0 ? (updatedAt ? `${tradingCount} 只 · ${hhmm(updatedAt)}` : `${tradingCount} 只`) : ""}
        </span>
        <button
          className="stock-icon-btn accent"
          title="添加自选股"
          onClick={(e) => {
            e.stopPropagation();
            setAddOpen(true);
          }}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.8" strokeLinecap="round">
            <path d="M12 5v14M5 12h14" />
          </svg>
        </button>
        <button
          className={`stock-icon-btn ${spinning ? "spin" : ""}`}
          title="立即刷新"
          onClick={(e) => {
            e.stopPropagation();
            refresh();
          }}
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M20 11a8 8 0 1 0-2.3 5.7" />
            <path d="M20 5v6h-6" />
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
            openWidgetMenu("stock").catch(() => {});
          }}
        >
          ⋮
        </button>
      </div>

      <div className="stock-list">
        {stocks.length === 0 ? (
          <div className="stock-empty">
            还没有自选股
            <div className="stock-empty-tip">点标题栏的 ＋ 或右键面板添加</div>
          </div>
        ) : (
          stocks.map((s) => {
            const q = quotes[s.secid];
            const t = tone(q?.changePct ?? null);
            return (
              <div className={`stock-row ${t}`} key={s.id}>
                <span className="stock-name" title={`${s.name || s.code} ${s.code}`}>
                  {q?.name || s.name || s.code}
                </span>
                <span className="stock-code">{s.code}</span>
                <span className="stock-price">{fmtPrice(q?.price ?? null)}</span>
                <span className="stock-pct">{fmtPct(q?.changePct ?? null)}</span>
                <button
                  className="stock-del"
                  title="从自选里移除"
                  tabIndex={-1}
                  onClick={(e) => {
                    e.stopPropagation();
                    doDelete(s.id);
                  }}
                >
                  ×
                </button>
              </div>
            );
          })
        )}
      </div>

      {err && (
        <div className="stock-err" title={err}>
          {err}
        </div>
      )}

      {/* 添加自选：默认收起，点标题栏 ＋ 或右键菜单才弹出 */}
      {addOpen && (
        <>
          <div className="stock-mask" onMouseDown={() => setAddOpen(false)} />
          <div className="stock-add-pop">
            <input
              className="stock-input"
              autoFocus
              value={input}
              placeholder="代码，如 600000 / 00700 / AAPL"
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  doAdd();
                }
                if (e.key === "Escape") setAddOpen(false);
              }}
            />
            <button
              className="stock-add-btn"
              title="添加自选"
              disabled={!input.trim() || busyAdd}
              onClick={doAdd}
            >
              {busyAdd ? "…" : "＋"}
            </button>
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
