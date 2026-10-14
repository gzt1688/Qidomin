/**
 * 组件菜单窗口 —— 独立置顶小窗口（便签 / 股票 / 黄历 / 诗词共用）。
 *
 * 为什么不做在组件里面：组件越矮，画在里面的菜单被裁得越狠，下面的项根本点不到。
 * 独立窗口没有这个问题，也不会被"沉到桌面层"的组件压住。
 *
 * 这里**只渲染**，不含任何业务逻辑：菜单长什么样由组件窗口通过事件发布过来
 * （见 `widgetMenu.ts`），点选再报回去，由组件自己执行。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  closeWidgetMenu,
  fitWidgetMenu,
  getMenuModel,
  requestModel,
  emitPick,
  EVT_MODEL,
  type MenuItem,
  type WidgetKind,
} from "../widgetMenu";
import { listen } from "@tauri-apps/api/event";

export default function WidgetMenuWindow({ kind }: { kind: WidgetKind }) {
  const [model, setModel] = useState<MenuItem[]>([]);
  /** 首屏内容到手了才显示：否则会先闪一条只有背景的"白条" */
  const [ready, setReady] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  /** 原生取色器会抢焦点导致本窗口失焦，这段时间内别把菜单自动收起 */
  const pickerGuard = useRef(0);
  /**
   * 滑杆 / 取色器的本地即时反馈。
   * 菜单窗和组件窗是两个窗口，滑块一拖要绕"菜单窗 → 组件 → 菜单窗"
   * 两跳 IPC 才能拿回新值；纯受控渲染会让滑块在这一圈没回来之前
   * 被旧值弹回去（看起来就是拖了但滑块不动）。先在本地动起来，
   * 组件回发的正式模型一到再清掉本地值对齐。
   */
  const [local, setLocal] = useState<
    Record<string, { color?: string; opacity?: number }>
  >({});

  // 索要菜单内容；组件每次状态变化都会重发一次
  useEffect(() => {
    let alive = true;
    let un: (() => void) | undefined;
    // 先向后端仓库直取一份（不靠事件时序，首屏就有内容），
    // 再请组件把最新的重发一遍（组件状态可能已经变了）。
    void getMenuModel(kind)
      .catch(() => [] as MenuItem[])
      .then((m) => {
        if (!alive) return;
        if (m.length) setModel(m);
        setReady(true);
      });
    requestModel(kind);
    void listen<{ kind: WidgetKind; model: MenuItem[] }>(EVT_MODEL, (e) => {
      if (e.payload?.kind !== kind) return;
      setModel(e.payload.model || []);
      setLocal({}); // 组件回发的正式值到了，本地即时反馈可以退场
      setReady(true);
    }).then((f) => {
      if (alive) un = f;
      else f();
    });
    return () => {
      alive = false;
      un?.();
    };
  }, [kind]);

  // 内容量完 → 让后端贴到组件外侧 + 定尺寸 + 显示，然后激活自己
  // （激活后点到别处才会 blur 自动收起）。内容高度一变就重新贴合，避免被裁。
  useEffect(() => {
    if (!ready) return;
    const el = rootRef.current;
    if (!el) return;
    let last = 0;
    const fit = (activate: boolean) => {
      const h = Math.max((el.scrollHeight || 320) + 2, 48);
      if (Math.abs(h - last) < 1) return;
      last = h;
      const p = fitWidgetMenu(kind, h);
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
  }, [kind, model, ready]);

  // 再次被打开时：重新索要内容（组件状态可能变了）+ 重新贴合
  useEffect(() => {
    const onFocus = () => {
      requestModel(kind);
      const h = Math.max((rootRef.current?.scrollHeight ?? 320) + 2, 48);
      fitWidgetMenu(kind, h).catch(() => {});
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [kind]);

  // 点到菜单外 / Esc → 收起
  useEffect(() => {
    const onBlur = () => {
      if (Date.now() - pickerGuard.current < 8000) return;
      closeWidgetMenu(kind).catch(() => {});
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeWidgetMenu(kind).catch(() => {});
    };
    window.addEventListener("blur", onBlur);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("keydown", onKey);
    };
  }, [kind]);

  /** 点选：报给组件窗口，由它执行（组件执行完会重发菜单内容） */
  const send = useCallback(
    (id: string, value?: string | number, close = true) => {
      // 乐观更新：组件窗在另一个窗口，事件绕一圈有延迟还可能丢——
      // 芯片类先在本地把选中态改了，UI 立即跟手；正式模型到了再覆盖
      if (value !== undefined) {
        setModel((prev) =>
          prev.map((it) =>
            it.t === "chips" && it.id === id
              ? { ...it, value: String(value) }
              : it
          )
        );
      }
      emitPick(kind, id, value);
      if (close) closeWidgetMenu(kind).catch(() => {});
    },
    [kind]
  );

  return (
    <div className="box-win-menu box-menu-window widget-menu-window" ref={rootRef}>
      {model.length === 0 && (
        <div className="menu-label">菜单加载中…（没出来就是组件还没发布）</div>
      )}
      {model.map((it, i) => {
        switch (it.t) {
          case "label":
            return (
              <div className="menu-label" key={`l${i}`}>
                {it.text}
              </div>
            );
          case "action":
            return (
              <button
                key={it.id}
                className={`${it.danger ? "menu-danger" : ""} ${
                  it.primary ? "menu-primary" : ""
                }`}
                onClick={() => send(it.id)}
              >
                {it.label}
              </button>
            );
          case "chips":
            return (
              <div className="menu-chips" key={it.id}>
                {it.options.map((o) => (
                  <button
                    key={o.value}
                    className={`menu-chip ${o.value === it.value ? "on" : ""}`}
                    // 换选项不收起菜单：还要接着调颜色 / 再点一次
                    onClick={() => send(it.id, o.value, false)}
                  >
                    {o.label}
                  </button>
                ))}
              </div>
            );
          case "color": {
            // 本地值优先（拖动中的即时反馈），组件回发的正式值兜底
            const v = local[it.id] || {};
            const color = v.color ?? it.color;
            const opacity = v.opacity ?? it.opacity;
            return (
              <div className="menu-bgrow" key={it.id}>
                <input
                  type="color"
                  className="color-input"
                  value={color}
                  // 原生取色器会弹系统对话框 → 本窗口失焦。记下时间，别把它当成"点到别处"
                  onClick={() => {
                    pickerGuard.current = Date.now();
                  }}
                  onFocus={() => {
                    pickerGuard.current = Date.now();
                  }}
                  onChange={(e) => {
                    setLocal((m) => ({
                      ...m,
                      [it.id]: { ...m[it.id], color: e.target.value },
                    }));
                    send(it.id + ":color", e.target.value, false);
                  }}
                />
                <input
                  type="range"
                  min={0}
                  max={100}
                  value={opacity}
                  onChange={(e) => {
                    const val = Number(e.target.value);
                    setLocal((m) => ({
                      ...m,
                      [it.id]: { ...m[it.id], opacity: val },
                    }));
                    send(it.id + ":opacity", val, false);
                  }}
                />
                <span className="menu-bgval">{opacity}%</span>
              </div>
            );
          }
          default:
            return null;
        }
      })}
    </div>
  );
}
