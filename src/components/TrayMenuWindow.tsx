import { useCallback, useEffect, useState, type ReactNode } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import {
  getSettings,
  setAutoStartEnabled,
  trayMenuAction,
  trayMenuDismiss,
  trayMenuPing,
} from "../api";

/** 心跳双通道：invoke（走命令权限域）+ emit（走事件权限域）。
 *  哪条到了日志里看得清 —— invoke 不通而 emit 通 = 权限/命令注册问题；
 *  两条都不通 = WebView 脚本没跑。 */
function heartbeat(reason: string) {
  trayMenuPing(reason).catch(() => {});
  emit("tray-menu-alive", reason).catch(() => {});
}

interface MenuItemDef {
  key: string;
  label: string;
  hint?: string;
  /** 简笔画图标：16px 内的线性 SVG（单色，随主题） */
  icon: ReactNode;
  danger?: boolean;
}

/** 分隔线占位 */
const SEP = "sep";

const ICONS = {
  toggle: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <rect x="2.2" y="3.2" width="5" height="4" rx="1" />
      <rect x="8.8" y="8.8" width="5" height="4" rx="1" />
      <path d="M7.2 5.2h3.4a1.6 1.6 0 0 1 1.6 1.6v1.4M8.8 10.8H5.4a1.6 1.6 0 0 1-1.6-1.6V7.8" />
    </svg>
  ),
  add: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <rect x="2.2" y="2.2" width="11.6" height="11.6" rx="2.2" />
      <path d="M8 5.4v5.2M5.4 8h5.2" />
    </svg>
  ),
  addRadial: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <circle cx="8" cy="8" r="2" />
      <circle cx="8" cy="2.8" r="1.35" />
      <circle cx="12.3" cy="5.4" r="1.35" />
      <circle cx="12.3" cy="10.6" r="1.35" />
      <circle cx="8" cy="13.2" r="1.35" />
      <circle cx="3.7" cy="10.6" r="1.35" />
      <circle cx="3.7" cy="5.4" r="1.35" />
    </svg>
  ),
  organize: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <path d="M2.5 4.5h7M2.5 8h11M11.5 11.5h2" />
      <path d="M9.5 2.5 11.5 4.5 9.5 6.5M13.5 9.5 11.5 11.5 13.5 13.5" />
    </svg>
  ),
  autostart: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <path d="M8 2.2a5.8 5.8 0 1 1-5.7 6.9" />
      <path d="M8 4.8V8l2.2 1.3" />
      <path d="M2 2.5v3h3" />
    </svg>
  ),
  settings: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <circle cx="8" cy="8" r="2.1" />
      <path d="M8 1.8v1.9M8 12.3v1.9M1.8 8h1.9M12.3 8h1.9M3.6 3.6l1.35 1.35M11.05 11.05l1.35 1.35M12.4 3.6l-1.35 1.35M4.95 11.05 3.6 12.4" />
    </svg>
  ),
  quit: (
    <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.4">
      <path d="M8 2.2v5" />
      <path d="M5.2 4.2a5 5 0 1 0 5.6 0" />
    </svg>
  ),
};

/**
 * 托盘右键菜单（Win11 Fluent 风格，自绘）。
 *
 * 窗口由后端 show_tray_menu 创建/复用：无边框透明置顶小窗，贴着托盘图标弹出。
 * 菜单项动作统一走 tray_menu_action 命令；点击后窗口自己 hide（进程不退，下次复用）。
 * 失焦自动收起 —— 托盘菜单的标配交互；Esc 同效。
 */
export default function TrayMenuWindow() {
  const [autoStart, setAutoStart] = useState<boolean | null>(null);

  // 收起 = 请后端把窗口挪到屏幕外（窗口常驻、WebView 不挂起）。
  // 绝不能 hide() 自己：hide 会让 WebView2 挂起渲染/脚本，下次弹出可能不渲染
  // （实测整轮"菜单失效"就是这么来的）。留 120ms 给收起动画播完。
  const hide = useCallback(() => {
    window.setTimeout(() => trayMenuDismiss().catch(() => {}), 120);
  }, []);

  const run = useCallback(
    (action: string) => {
      // 先收起再执行：退出动作会销毁进程，菜单窗口必须先藏好；
      // 其他动作收起后执行也符合菜单的直觉
      hide();
      trayMenuAction(action).catch((e) => {
        // invoke 失败不能只打 console（谁也看不见），回传后端日志留痕
        console.error("托盘菜单动作失败", e);
        heartbeat(`动作 ${action} 失败：${e}`);
      });
    },
    [hide]
  );

  const loadAutoStart = useCallback(() => {
    getSettings()
      .then((s) => setAutoStart(s.autoStart))
      .catch(() => {});
  }, []);

  useEffect(() => {
    loadAutoStart();
    // 心跳：React 挂载完成 —— 证明 WebView 加载成功（预创建后启动期打一次）
    heartbeat("挂载完成");
    // 定时心跳：WebView 脚本的"活着"证明。实测过脚本会被静默冻死
    // （窗口保留最后一帧渲染，看着正常但点击/事件全无响应）——
    // 后端弹出时检查心跳龄，断流即销毁重建，实现自愈。
    const beat = window.setInterval(() => heartbeat("定时"), 10_000);
    // 每次后端把窗口挪回屏幕弹出时刷新：重读自启勾选态；
    // 这条事件回执同时是"WebView 常驻活着"的探针（日志可见）
    const un = listen<number>("tray-menu-shown", () => {
      loadAutoStart();
      heartbeat("弹出刷新");
    });
    return () => {
      window.clearInterval(beat);
      un.then((f) => f());
    };
  }, [loadAutoStart]);

  // Esc 收起。失焦自动收起由后端轮询负责（spawn_tray_menu_blur_watcher）——
  // WebView 的前端焦点事件在托盘点击场景经常不触发，不能依赖。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [hide]);

  const toggleAutoStart = useCallback(() => {
    const next = !(autoStart ?? false);
    setAutoStart(next); // 乐观更新，注册表写入很快，失败概率低
    setAutoStartEnabled(next).catch((e) => {
      heartbeat(`切换开机自启失败：${e}`);
      setAutoStart(!next);
    });
  }, [autoStart]);

  const items: (MenuItemDef | typeof SEP)[] = [
    { key: "toggle", label: "显示 / 隐藏全部分区", icon: ICONS.toggle },
    { key: "add", label: "新建分区", icon: ICONS.add },
    { key: "addRadial", label: "新建饼型菜单", icon: ICONS.addRadial },
    { key: "organize", label: "一键整理", hint: "预览后移动", icon: ICONS.organize },
    {
      key: "autostart",
      label: "开机自启",
      hint: autoStart == null ? "" : autoStart ? "已开启" : "已关闭",
      icon: ICONS.autostart,
    },
    SEP,
    { key: "settings", label: "设置", icon: ICONS.settings },
    SEP,
    { key: "quit", label: "退出", icon: ICONS.quit, danger: true },
  ];

  return (
    <div className="tmenu-root" onContextMenu={(e) => e.preventDefault()}>
      <div className="tmenu">
        {items.map((it, i) =>
          it === SEP ? (
            <div key={`sep-${i}`} className="tmenu-sep" />
          ) : (
            <button
              key={it.key}
              className={`tmenu-item${it.danger ? " danger" : ""}`}
              onClick={() => (it.key === "autostart" ? toggleAutoStart() : run(it.key))}
            >
              <span className="tmenu-icon">{it.icon}</span>
              <span className="tmenu-label">{it.label}</span>
              {it.hint ? <span className="tmenu-hint">{it.hint}</span> : null}
              {it.key === "autostart" && (
                <span className={`tmenu-check${autoStart ? " on" : ""}`} aria-hidden>
                  {autoStart ? "✓" : ""}
                </span>
              )}
            </button>
          )
        )}
      </div>
    </div>
  );
}
