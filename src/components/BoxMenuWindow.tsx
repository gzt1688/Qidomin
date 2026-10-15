import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  boxMenuAction,
  closeBoxMenu,
  fitBoxMenu,
  getBoxById,
  getBoxWindowCfg,
  getSettings,
} from "../api";
import type { BoxInfo, BoxWindowCfg } from "../types";

const ICON_SIZES = [24, 32, 48, 64, 96];
const SORT_MODES = [
  { key: "name", label: "名称" },
  { key: "time", label: "修改时间" },
  { key: "size", label: "大小" },
  { key: "type", label: "类型" },
];

/**
 * 分区菜单 —— 独立置顶小窗口。
 *
 * 为什么不做在分区窗口里：菜单会被窗口边界裁掉（分区越小越看不全），而且分区现在
 * 被沉到"桌面层"，画在里面的菜单还会被别的程序盖住。独立窗口两个问题一起解决。
 *
 * 这里只负责渲染 + 表达意图，真正的业务逻辑全在 BoxWindow 里（通过事件转交）。
 */
export default function BoxMenuWindow({ boxId }: { boxId: number }) {
  const [box, setBox] = useState<BoxInfo | null>(null);
  const [cfg, setCfg] = useState<BoxWindowCfg | null>(null);
  const [bg, setBg] = useState({ color: "#ffffff", opacity: 0 });
  const [ready, setReady] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  /** 原生取色器会抢焦点导致本窗口失焦，这段时间内别把菜单自动收起来 */
  const pickerGuard = useRef(0);

  const reload = useCallback(async () => {
    try {
      const [b, c, s] = await Promise.all([
        getBoxById(boxId),
        getBoxWindowCfg(boxId),
        getSettings(),
      ]);
      setBox(b);
      setCfg(c);
      setBg({
        color: c?.bgColor || s.boxBgColor || "#ffffff",
        opacity: c?.bgOpacity ?? s.boxBgOpacity ?? 0,
      });
    } catch {
      /* 读不到就按默认渲染，不阻塞菜单出现 */
    } finally {
      setReady(true);
    }
  }, [boxId]);

  useEffect(() => {
    reload();
  }, [reload]);

  // 内容量完 → 让后端按分区位置摆正 + 定尺寸 + 显示，然后把自己激活
  // （激活后点到别处才能收到 blur 自动收起）。
  // 用 ResizeObserver 盯着内容高度：切图标/列表、显示不同分组都会变高变矮，
  // 高度一变就重新贴合窗口尺寸，否则内容会被窗口边界裁掉。
  useEffect(() => {
    if (!ready) return;
    const el = rootRef.current;
    if (!el) return;
    let last = 0;
    const fit = (activate: boolean) => {
      const h = (el.scrollHeight || 320) + 2;
      if (Math.abs(h - last) < 1) return;
      last = h;
      const p = fitBoxMenu(boxId, h);
      if (activate) {
        p.then(() => getCurrentWindow().setFocus().catch(() => {})).catch(() => {});
      } else {
        p.catch(() => {});
      }
    };
    fit(true);
    const ro = new ResizeObserver(() => fit(false));
    ro.observe(el);
    return () => ro.disconnect();
  }, [ready, boxId]);

  // 再次被切换打开时：刷新状态 + 重新贴到分区外侧（分区可能已经移动/改变大小）
  useEffect(() => {
    const onFocus = () => {
      void reload();
      const h = (rootRef.current?.scrollHeight ?? 320) + 2;
      fitBoxMenu(boxId, h).catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [reload, boxId]);

  // 点到菜单外 / 切到别的程序 → 自动收起；Esc 也可以关
  useEffect(() => {
    const onBlur = () => {
      if (Date.now() - pickerGuard.current < 8000) return;
      closeBoxMenu(boxId).catch(() => {});
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeBoxMenu(boxId).catch(() => {});
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("keydown", onKey);
    };
  }, [boxId]);

  /** 把操作转交给分区窗口执行；close=false 用于"改完配置刷新一下、菜单不收起" */
  const send = useCallback(
    (action: Record<string, unknown>, close = true) => {
      boxMenuAction(boxId, action, close).catch(() => {});
    },
    [boxId]
  );

  /** 调整图标网格间距（步进 2px，范围 0-40） */
  const stepGap = useCallback(
    (dx: number, dy: number) => {
      const gx = Math.min(40, Math.max(0, (cfg?.gapX ?? 6) + dx));
      const gy = Math.min(40, Math.max(0, (cfg?.gapY ?? 6) + dy));
      setCfg((c) => (c ? { ...c, gapX: gx, gapY: gy } : c));
      send({ kind: "gap", gapX: gx, gapY: gy }, false);
    },
    [cfg, send]
  );

  const isFolder = box?.boxType === "folder";
  const isList = cfg?.viewMode === "list";
  const isLocked = cfg?.locked === true;

  return (
    <div ref={rootRef} className="box-win-menu box-menu-window">
      <div className="menu-label">新建分区</div>
      <div className="menu-row">
        <button
          className="menu-primary"
          title="新建文件分区：任何文件、图标、快捷方式都能放"
          onClick={() => send({ kind: "create", boxType: "file" })}
        >
          文件
        </button>
        <button title="新建一个关联到某个文件夹的分区" onClick={() => send({ kind: "newFolder" })}>
          文件夹
        </button>
      </div>

      <div className="menu-row menu-row-sep">
        <button
          className={isLocked ? "" : "active"}
          title="解锁：可以拖动、可以调整大小"
          onClick={() => {
            setCfg((c) => (c ? { ...c, locked: false } : c));
            send({ kind: "locked", value: false }, false);
          }}
        >
          解锁
        </button>
        <button
          className={isLocked ? "active" : ""}
          title="锁定：位置和大小固定，避免误拖"
          onClick={() => {
            setCfg((c) => (c ? { ...c, locked: true } : c));
            send({ kind: "locked", value: true }, false);
          }}
        >
          锁定
        </button>
        {cfg?.collapsed ? (
          <button
            title="展开：恢复收起前的大小和位置"
            onClick={() => {
              setCfg((c) => (c ? { ...c, collapsed: false } : c));
              send({ kind: "expand" }, false);
            }}
          >
            展开
          </button>
        ) : (
          <button
            title="收起为胶囊：点击或悬停可再展开"
            onClick={() => {
              setCfg((c) => (c ? { ...c, collapsed: true } : c));
              send({ kind: "collapse" }, false);
            }}
          >
            收起
          </button>
        )}
      </div>
      {/* 胶囊模式下才需要"悬停展开"这个开关：普通分区悬停没有展开行为 */}
      {cfg?.collapsed ? (
        <div className="menu-row">
          <button
            className={cfg?.capsuleHover === false ? "" : "active"}
            title="关：鼠标悬停胶囊不自动展开，只能点击展开"
            onClick={() => {
              setCfg((c) => (c ? { ...c, capsuleHover: false } : c));
              send({ kind: "capsuleHover", value: false }, false);
            }}
          >
            悬停不展开
          </button>
          <button
            className={cfg?.capsuleHover === false ? "active" : ""}
            title="开（默认）：鼠标悬停胶囊 0.5 秒自动临时展开，移开后收起"
            onClick={() => {
              setCfg((c) => (c ? { ...c, capsuleHover: true } : c));
              send({ kind: "capsuleHover", value: true }, false);
            }}
          >
            悬停展开
          </button>
        </div>
      ) : null}

      <div className="menu-label">显示方式</div>
      <div className="menu-row">
        <button
          className={isList ? "" : "active"}
          title="大图标网格"
          onClick={() => {
            setCfg((c) => (c ? { ...c, viewMode: "icon" } : c));
            send({ kind: "viewMode", value: "icon" }, false);
          }}
        >
          图标
        </button>
        <button
          className={isList ? "active" : ""}
          title="竖排列表：一行一个文件，适合文件多、名字长"
          onClick={() => {
            setCfg((c) => (c ? { ...c, viewMode: "list" } : c));
            send({ kind: "viewMode", value: "list" }, false);
          }}
        >
          列表
        </button>
      </div>

      {/* 图标大小只对图标网格有意义，列表模式下不显示 */}
      {!isList && (
        <>
          <div className="menu-label">图标大小</div>
          <div className="menu-row">
            {ICON_SIZES.map((s) => (
              <button
                key={s}
                className={cfg?.iconSize === s ? "active" : ""}
                onClick={() => {
                  setCfg((c) => (c ? { ...c, iconSize: s } : c));
                  send({ kind: "iconSize", value: s }, false);
                }}
              >
                {s}
              </button>
            ))}
          </div>

          <div className="menu-label">图标间距</div>
          <div className="menu-row">
            <span className="menu-gap-tag">横</span>
            <button onClick={() => stepGap(-2, 0)}>−</button>
            <span className="menu-bgval" style={{ flex: 1, minWidth: 0, textAlign: "center" }}>
              {cfg?.gapX ?? 6}
            </span>
            <button onClick={() => stepGap(2, 0)}>＋</button>
            <span className="menu-gap-tag">竖</span>
            <button onClick={() => stepGap(0, -2)}>−</button>
            <span className="menu-bgval" style={{ flex: 1, minWidth: 0, textAlign: "center" }}>
              {cfg?.gapY ?? 6}
            </span>
            <button onClick={() => stepGap(0, 2)}>＋</button>
          </div>
        </>
      )}

      <div className="menu-label">外观（本分区）</div>
      <div className="menu-bgrow">
        <input
          type="color"
          className="color-input"
          value={bg.color}
          onMouseDown={() => {
            pickerGuard.current = Date.now();
          }}
          onChange={(e) => {
            pickerGuard.current = 0;
            const color = e.target.value;
            setBg((b) => ({ ...b, color }));
            send({ kind: "bg", color, opacity: bg.opacity }, false);
          }}
        />
        <input
          type="range"
          min={0}
          max={100}
          value={bg.opacity}
          onChange={(e) => {
            const opacity = Number(e.target.value);
            setBg((b) => ({ ...b, opacity }));
            send({ kind: "bg", color: bg.color, opacity }, false);
          }}
        />
        <span className="menu-bgval">{bg.opacity}%</span>
      </div>

      <div className="menu-row">
        <button
          className={cfg?.labelColor === "#000000" ? "" : "active"}
          title="分区里的文件名用白色"
          onClick={() => {
            setCfg((c) => (c ? { ...c, labelColor: "#ffffff" } : c));
            send({ kind: "labelColor", value: "#ffffff" }, false);
          }}
        >
          白字
        </button>
        <button
          className={cfg?.labelColor === "#000000" ? "active" : ""}
          title="分区里的文件名用黑色"
          onClick={() => {
            setCfg((c) => (c ? { ...c, labelColor: "#000000" } : c));
            send({ kind: "labelColor", value: "#000000" }, false);
          }}
        >
          黑字
        </button>
      </div>

      {isFolder && (
        <>
          <div className="menu-label">排序</div>
          <div className="menu-row">
            {SORT_MODES.map((m) => (
              <button
                key={m.key}
                className={box?.sortMode === m.key ? "active" : ""}
                onClick={() => {
                  setBox((b) => (b ? { ...b, sortMode: m.key } : b));
                  send({ kind: "sort", value: m.key }, false);
                }}
              >
                {m.label}
              </button>
            ))}
          </div>
          <button onClick={() => send({ kind: "linkFolder" })}>
            {box?.linkPath ? "更改链接文件夹" : "设置链接文件夹"}
          </button>
        </>
      )}

      {!isFolder && (
        <div className="menu-row">
          <button title="把桌面上同类文件吸附进本分区" onClick={() => send({ kind: "absorb" })}>
            吸附
          </button>
          <button title="按规则把本分区的文件整理到其它分区" onClick={() => send({ kind: "organize" })}>
            整理
          </button>
        </div>
      )}

      <button
        className="menu-danger"
        title={
          isFolder
            ? "解散这个快捷访问（关联的文件夹和里面的文件保持原样）"
            : "解散分区：文件按原来的位置退回，不会被删除"
        }
        // 直接解散，不再弹确认框。普通分区里的文件会**按原路退回**（原来在桌面就回桌面，
        // 原来在哪个文件夹就回哪个文件夹），文件本身还是原来的图标，不改名不删除。
        onClick={() => send({ kind: "delete" })}
      >
        解散分区
      </button>
    </div>
  );
}
