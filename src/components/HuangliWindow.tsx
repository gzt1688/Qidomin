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
import { Solar, type JieQi } from "lunar-javascript";
import {
  getHuangliWindowCfg,
  saveHuangliWindowCfg,
  setHuangliEnabled,
  startHuangliDrag,
} from "../api";
import type { HuangliWindowCfg } from "../types";
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
  if (!m) return `rgba(255, 253, 243,${alpha})`;
  const n = parseInt(m[1], 16);
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
}

const DEFAULT_CFG: HuangliWindowCfg = {
  x: 300,
  y: 200,
  w: 300,
  h: 470,
  locked: false,
  visible: true,
  bgColor: "#fffdf3",
  bgOpacity: 96,
};

const pad2 = (n: number) => String(n).padStart(2, "0");

interface JieQiInfo {
  name: string;
  /** MM-DD */
  date: string;
  /** 相对今天的天数：负=已过去，正=还没到 */
  days: number;
}

interface HuangliData {
  day: number;
  year: number;
  month: number;
  week: string;
  xingZuo: string;
  lunarText: string;
  yearGanZhi: string;
  monthGanZhi: string;
  dayGanZhi: string;
  shengXiao: string;
  season: string;
  /** 当天正好是节气时是节气名，否则空串 */
  jieQiToday: string;
  prev: JieQiInfo;
  next: JieQiInfo;
  yi: string[];
  ji: string[];
  jiShen: string[];
  xiongSha: string[];
  tianShen: string;
  tianShenType: string;
  tianShenLuck: string;
  zhiXing: string;
  xiu: string;
  xiuLuck: string;
  chong: string;
  sha: string;
  cai: string;
  xi: string;
  fu: string;
  yangGui: string;
  yinGui: string;
  pengZuGan: string;
  pengZuZhi: string;
  festivals: string[];
}

/** 把一天的公历/农历/节气/宜忌/神煞一次算齐 */
function buildHuangli(date: Date): HuangliData {
  const solar = Solar.fromDate(date);
  const lunar = solar.getLunar();

  const todayUtc = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
  // getMonth() 是 1-12，转成 Date.UTC 需要减 1
  const toUtc = (s: { getYear(): number; getMonth(): number; getDay(): number }) =>
    Date.UTC(s.getYear(), s.getMonth() - 1, s.getDay());
  const solarInfo = (jq: JieQi): JieQiInfo => {
    const s = jq.getSolar();
    return {
      name: jq.getName(),
      date: `${pad2(s.getMonth())}-${pad2(s.getDay())}`,
      days: Math.round((toUtc(s) - todayUtc) / 86400000),
    };
  };

  const jieQiToday = lunar.getJieQi() || "";
  let next = solarInfo(lunar.getNextJieQi());
  // 当天正好是节气时 getNextJieQi() 返回的是它自己，得再从节气表里往后顺延一个
  if (jieQiToday && next.days <= 0) {
    const later = Object.entries(lunar.getJieQiTable())
      .map(([name, s]) => ({ name, s, t: toUtc(s) }))
      .filter((e) => e.t > todayUtc)
      .sort((a, b) => a.t - b.t)[0];
    if (later) {
      next = {
        name: later.name,
        date: `${pad2(later.s.getMonth())}-${pad2(later.s.getDay())}`,
        days: Math.round((later.t - todayUtc) / 86400000),
      };
    }
  }

  const festivals = Array.from(
    new Set([...solar.getFestivals(), ...lunar.getFestivals()])
  );

  return {
    day: solar.getDay(),
    year: solar.getYear(),
    month: solar.getMonth(),
    week: `星期${solar.getWeekInChinese()}`,
    xingZuo: `${solar.getXingZuo()}座`,
    lunarText: `${lunar.getMonthInChinese()}月${lunar.getDayInChinese()}`,
    yearGanZhi: `${lunar.getYearInGanZhi()}年`,
    monthGanZhi: `${lunar.getMonthInGanZhi()}月`,
    dayGanZhi: `${lunar.getDayInGanZhi()}日`,
    shengXiao: lunar.getYearShengXiao(),
    season: lunar.getSeason(),
    jieQiToday,
    prev: solarInfo(lunar.getPrevJieQi()),
    next,
    yi: lunar.getDayYi(),
    ji: lunar.getDayJi(),
    jiShen: lunar.getDayJiShen(),
    xiongSha: lunar.getDayXiongSha(),
    tianShen: lunar.getDayTianShen(),
    tianShenType: lunar.getDayTianShenType(),
    tianShenLuck: lunar.getDayTianShenLuck(),
    zhiXing: lunar.getZhiXing(),
    xiu: lunar.getXiu(),
    xiuLuck: lunar.getXiuLuck(),
    chong: lunar.getDayChongDesc(),
    sha: lunar.getDaySha(),
    cai: lunar.getDayPositionCaiDesc(),
    xi: lunar.getDayPositionXiDesc(),
    fu: lunar.getDayPositionFuDesc(),
    yangGui: lunar.getDayPositionYangGuiDesc(),
    yinGui: lunar.getDayPositionYinGuiDesc(),
    pengZuGan: lunar.getPengZuGan(),
    pengZuZhi: lunar.getPengZuZhi(),
    festivals,
  };
}

function ChipGroup({ items, tone }: { items: string[]; tone: "yi" | "ji" }) {
  if (items.length === 0) {
    return <span className={`hl-chip ${tone} empty`}>无</span>;
  }
  return (
    <>
      {items.map((t) => (
        <span className={`hl-chip ${tone}`} key={t}>
          {t}
        </span>
      ))}
    </>
  );
}

export default function HuangliWindow() {
  const [cfg, setCfg] = useState<HuangliWindowCfg>(DEFAULT_CFG);
  const [today, setToday] = useState(() => new Date());
  /** 相对今天的偏移：0=今天，-1=前一天，1=后一天 */
  const [offset, setOffset] = useState(0);

  // 首次加载窗口配置
  useEffect(() => {
    getHuangliWindowCfg()
      .then(setCfg)
      .catch(() => {
        /* 用默认值即可 */
      });
  }, []);

  // 跨零点自动翻页（黄历最怕停在一整天前的日期上）
  useEffect(() => {
    const id = window.setInterval(() => {
      const now = new Date();
      setToday((prev) => (now.toDateString() === prev.toDateString() ? prev : now));
    }, 60 * 1000);
    return () => window.clearInterval(id);
  }, []);

  const targetDate = useMemo(() => {
    const d = new Date(today);
    d.setDate(d.getDate() + offset);
    return d;
  }, [today, offset]);

  // 历法推算全是纯函数，换个日期重算一次即可
  const data = useMemo(() => buildHuangli(targetDate), [targetDate]);

  // cfg 的 ref 镜像：updateCfg 要在 setState updater 之外读到最新值
  const cfgRef = useRef(cfg);
  useEffect(() => {
    cfgRef.current = cfg;
  }, [cfg]);

  const updateCfg = useCallback((p: Partial<HuangliWindowCfg>) => {
    // 副作用放在 updater 外面：严格模式下 updater 会被调用两次，
    // 写在里面等于一次操作发两次落盘 IPC
    const next = { ...cfgRef.current, ...p };
    cfgRef.current = next;
    setCfg(next);
    saveHuangliWindowCfg(next).catch(() => {});
  }, []);

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
        fetchSnapQuietly("huangli"),
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
    if (ds.moved) startHuangliDrag(true).catch(() => {});
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
  } | null>(null);
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
        fetchSnapQuietly("huangli"),
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
      let nw = Math.max(240, Math.round(s.w + (s.lastX - s.sx) * s.scale));
      let nh = Math.max(200, Math.round(s.h + (s.lastY - s.sy) * s.scale));
      // 拉伸吸附：右/下边缘在阈值内就贴到候选边上（其它分区/组件/屏幕边缘）
      [nw, nh] = resizeSnap(s.px, s.py, nw, nh, s.targets, s.threshold);
      nw = Math.max(240, nw);
      nh = Math.max(200, nh);
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
    setHuangliEnabled(false).catch(() => {});
  }, []);

  const alpha = Math.min(1, Math.max(0, cfg.bgOpacity / 100));
  const bgStyle = {
    "--hl-bg": hexToRgba(cfg.bgColor, alpha),
    "--hl-bg-solid": hexToRgba(cfg.bgColor, Math.min(1, alpha + 0.04)),
  } as CSSProperties;

  // ---------- 右键菜单：发布给独立菜单窗口渲染（不再画在组件里被裁掉） ----------
  const menuModel = useCallback(
    (): MenuItem[] => [
      { t: "action", id: "close", label: "关闭组件", danger: true },
      { t: "label", text: "日期" },
      {
        t: "chips",
        id: "date",
        value: offset === 0 ? "0" : "",
        options: [
          { value: "-1", label: "前一天" },
          { value: "0", label: "今天" },
          { value: "1", label: "后一天" },
        ],
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
    [offset, cfg.bgColor, cfg.bgOpacity, cfg.locked]
  );

  useEffect(() => {
    publishMenu("huangli", menuModel());
  }, [menuModel]);

  useEffect(() => {
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;
    // 依赖变化太快时 listen 的订阅可能晚于清理完成——这时要立刻退订，
    // 否则旧监听器永远退不掉、越积越多（一次点击被当成 N 次切换）
    let disposed = false;
    void onMenuRequest("huangli", () => publishMenu("huangli", menuModel())).then(
      (f) => {
        if (disposed) f();
        else off1 = f;
      }
    );
    void onMenuPick("huangli", (id, value) => {
      switch (id) {
        case "close":
        case "hide":
          hideWindow();
          break;
        case "date":
          // -1 / +1 是在当前偏移上挪一天；0 是回到今天
          if (value === "0") setOffset(0);
          else if (value === "-1") setOffset((o) => o - 1);
          else if (value === "1") setOffset((o) => o + 1);
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
  }, [menuModel, hideWindow, updateCfg, cfg.locked, cfg.bgColor, cfg.bgOpacity]);

  /** 距上一个节气第几天（节气当天算第 1 天） */
  const termDay = -data.prev.days + 1;
  const dirs: [string, string][] = [
    ["财神", data.cai],
    ["喜神", data.xi],
    ["福神", data.fu],
    ["阳贵", data.yangGui],
    ["阴贵", data.yinGui],
  ];

  return (
    <div
      className={`huangli-window ${cfg.locked ? "locked" : ""}`}
      style={bgStyle}
      onContextMenu={(e) => {
        e.preventDefault();
        // 菜单是独立置顶窗口：组件再矮也裁不到它
        openWidgetMenu("huangli").catch(() => {});
      }}
    >
      <div
        className="box-win-head"
        onPointerDown={onHeaderDown}
        onPointerMove={onHeaderMove}
        onPointerUp={onHeaderUp}
      >
        <span className="box-win-dot" style={{ background: "#c0392b" }} />
        <span className="box-win-title">黄历</span>
        <span className="box-win-count" title={data.lunarText}>
          {data.lunarText}
        </span>
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
            openWidgetMenu("huangli").catch(() => {});
          }}
        >
          ⋮
        </button>
      </div>

      <div className="hl-body">
        {/* 公历 + 农历 */}
        <div className="hl-hero">
          <div className="hl-daynum">{data.day}</div>
          <div className="hl-hero-info">
            <div className="hl-hero-date">
              {data.year}年{data.month}月 · {data.week}
            </div>
            <div className="hl-hero-lunar">{data.lunarText}</div>
            <div className="hl-hero-sub">
              {data.xingZuo} · {data.season}
            </div>
          </div>
        </div>

        {offset !== 0 && (
          <button className="hl-backtoday" onClick={() => setOffset(0)}>
            ← 回到今天
          </button>
        )}

        {/* 干支纪年 / 月 / 日 */}
        <div className="hl-gz">
          <span>{data.yearGanZhi}</span>
          <span>{data.monthGanZhi}</span>
          <span className="hl-gz-day">{data.dayGanZhi}</span>
          <i>生肖{data.shengXiao}</i>
        </div>

        {/* 节气 */}
        <div className="hl-term">
          <div className="hl-term-row">
            <span className="hl-term-tag">{data.jieQiToday ? "今日节气" : data.prev.name}</span>
            <span className="hl-term-txt">
              {data.jieQiToday
                ? `${data.jieQiToday} · 今日交节`
                : `${data.prev.date} 交节 · 第 ${termDay} 天`}
            </span>
          </div>
          <div className="hl-term-row">
            <span className="hl-term-tag next">下一节气</span>
            <span className="hl-term-txt">
              {data.next.name} {data.next.date} · 还有 {data.next.days} 天
            </span>
          </div>
        </div>

        {/* 宜 / 忌 */}
        <div className="hl-yj">
          <div className="hl-yj-row">
            <span className="hl-key yi">宜</span>
            <div className="hl-chips">
              <ChipGroup items={data.yi} tone="yi" />
            </div>
          </div>
          <div className="hl-yj-row">
            <span className="hl-key ji">忌</span>
            <div className="hl-chips">
              <ChipGroup items={data.ji} tone="ji" />
            </div>
          </div>
        </div>

        {/* 值神 / 值星 / 星宿 / 冲煞 */}
        <div className="hl-grid">
          <div className="hl-cell">
            <span>值神</span>
            <b>{data.tianShen}</b>
            <i className={data.tianShenLuck === "吉" ? "good" : "bad"}>
              {data.tianShenType}
              {data.tianShenLuck}
            </i>
          </div>
          <div className="hl-cell">
            <span>值星</span>
            <b>{data.zhiXing}</b>
          </div>
          <div className="hl-cell">
            <span>星宿</span>
            <b>{data.xiu}</b>
            <i className={data.xiuLuck === "吉" ? "good" : "bad"}>{data.xiuLuck}</i>
          </div>
          <div className="hl-cell">
            <span>冲煞</span>
            <b>
              冲{data.chong} 煞{data.sha}
            </b>
          </div>
        </div>

        {/* 方位 */}
        <div className="hl-dirs">
          {dirs.map(([label, value]) => (
            <div className="hl-dir" key={label}>
              <span>{label}</span>
              <b>{value}</b>
            </div>
          ))}
        </div>

        {/* 吉神 / 凶煞 */}
        <div className="hl-ss">
          <div className="hl-ss-row">
            <i>吉神宜趋</i>
            <em>{data.jiShen.join(" · ") || "无"}</em>
          </div>
          <div className="hl-ss-row xiong">
            <i>凶煞宜忌</i>
            <em>{data.xiongSha.join(" · ") || "无"}</em>
          </div>
        </div>

        {/* 彭祖百忌 */}
        <div className="hl-pz">
          <i>彭祖百忌</i>
          <div>{data.pengZuGan}</div>
          <div>{data.pengZuZhi}</div>
        </div>

        {data.festivals.length > 0 && (
          <div className="hl-fest">
            {data.festivals.map((f) => (
              <span className="hl-fest-tag" key={f}>
                {f}
              </span>
            ))}
          </div>
        )}
      </div>

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
