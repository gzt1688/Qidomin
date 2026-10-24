import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { save as saveDialog, open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  applyShortcutArrowForced,
  clearIndex,
  exportLayout,
  getLastOrganizeBatch,
  getHuangdiEnabled,
  getHuangliEnabled,
  getNotesEnabled,
  getPoetryEnabled,
  getSettings,
  getShanghanEnabled,
  getShortcutArrowHidden,
  getStocksEnabled,
  importLayout,
  listDrives,
  rebuildSearchIndex,
  saveSettings,
  setHuangdiEnabled,
  setHuangliEnabled,
  setNotesEnabled,
  setPoetryEnabled,
  setShanghanEnabled,
  setShortcutArrowHidden,
  setStocksEnabled,
  startMouseCapture,
  undoLastOrganize,
} from "../api";
import { FILE_CATEGORIES } from "../fileCategories";
import type { AppSettings, MouseBinding } from "../types";
import { applyBoxTitleColor } from "../theme";

interface SettingsViewProps {
  notify: (text: string, kind: "ok" | "err" | "") => void;
  refreshKey: number;
  onChange: (updater: (k: number) => number) => void;
}

/** 设置页里的"撤销上次整理"小行：显示最近批次信息，点击原路搬回 */
function UndoOrganizeRow({
  notify,
}: {
  notify: (text: string, kind: "ok" | "err" | "") => void;
}) {
  const [info, setInfo] = useState<[string, number, string] | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    getLastOrganizeBatch()
      .then(setInfo)
      .catch(() => {});
  }, []);

  const doUndo = useCallback(async () => {
    setBusy(true);
    try {
      const [back, total] = await undoLastOrganize();
      notify(`已撤销：${back}/${total} 个文件回到原位`, back === total ? "ok" : "err");
      setInfo(null);
    } catch (e) {
      notify(`撤销失败：${String(e)}`, "err");
    } finally {
      setBusy(false);
    }
  }, [notify]);

  if (!info) {
    return <span style={{ fontSize: 12, color: "var(--text-sub)" }}>最近没有整理记录</span>;
  }
  return (
    <span className="radio-row" style={{ alignItems: "center" }}>
      <span style={{ fontSize: 12, color: "var(--text-sub)" }}>
        {info[1]} 个文件（{info[2]}）
      </span>
      <button className="btn small" disabled={busy} onClick={doUndo}>
        {busy ? "撤销中…" : "撤销"}
      </button>
    </span>
  );
}

const APP_VERSION = "V1.3-2026-10-15";

type TabKey = "basic" | "search" | "radial" | "widgets" | "hotkeys" | "about";

const TABS: { key: TabKey; label: string; hint: string }[] = [
  { key: "basic", label: "常规设置", hint: "外观 / 系统（布局备份在左下角）" },
  { key: "search", label: "搜索", hint: "索引范围 / 文件类型类目" },
  { key: "radial", label: "饼型菜单", hint: "圆钮尺寸 / 弹出尺寸 / 标题 / 图标" },
  { key: "widgets", label: "桌面组件", hint: "便签 / 股票 / 黄历 / 诗词 / 黄帝内经 / 伤寒论" },
  { key: "hotkeys", label: "鼠标快捷键", hint: "鼠标侧键 / 中键 → 功能" },
  { key: "about", label: "关于", hint: "版本与介绍" },
];

/**
 * 饼型菜单内置图标：与饼型右键菜单共用一份（src/radialIcons.ts）。
 */
import { RADIAL_PRESET_ICONS, svgToDataUrl } from "../radialIcons";

// ---------- 鼠标快捷键 ----------

/** 可绑定的鼠标按键（Windows 标准键） */
const MOUSE_BUTTONS: { key: string; label: string }[] = [
  { key: "middle", label: "中键" },
  { key: "middle2", label: "双击中键" },
  { key: "x1", label: "侧键1（前进）" },
  { key: "x2", label: "侧键2（后退）" },
];

/** 可映射的功能列表 */
const MOUSE_ACTIONS: { key: string; label: string; combo?: string }[] = [
  { key: "selectAll", label: "全选", combo: "Ctrl+A" },
  { key: "copy", label: "复制", combo: "Ctrl+C" },
  { key: "cut", label: "剪切", combo: "Ctrl+X" },
  { key: "paste", label: "粘贴", combo: "Ctrl+V" },
  { key: "undo", label: "撤销", combo: "Ctrl+Z" },
  { key: "redo", label: "重做", combo: "Ctrl+Y" },
  { key: "save", label: "保存", combo: "Ctrl+S" },
  { key: "delete", label: "删除", combo: "Delete" },
  { key: "refresh", label: "刷新", combo: "F5" },
  { key: "toggleBoxes", label: "显示 / 隐藏全部分区" },
  { key: "toggleSearch", label: "显示 / 隐藏快速搜索" },
  { key: "newBox", label: "新建分区" },
  { key: "organize", label: "一键整理" },
  { key: "toggleNotes", label: "显示 / 隐藏便签" },
];

function mouseButtonLabel(key: string): string {
  return MOUSE_BUTTONS.find((b) => b.key === key)?.label ?? key;
}

/**
 * 这个按键能不能用「键 + 空格」组合。
 *
 * 与 Rust 侧 `mousehotkey::has_chord_binding` 里的 `is_mouse_button` 保持一致：
 * 只有中键 / 侧键1 / 侧键2。**「双击中键」不行** ——
 * 它在钩子那边走的是另一条分支，选了组合模式会"绑上了却永远不触发"，
 * 所以下拉里把这一项禁用掉，别让人选到一个不工作的状态。
 */
function isChordableButton(key: string): boolean {
  return key === "middle" || key === "x1" || key === "x2";
}

function bindingLabel(b: MouseBinding): string {
  // 「鼠标键 + 空格」组合：不分先后，所以用 + 而不是 →（箭头会让人以为有先后）
  if (b.mode === "spaceAfter") {
    const mods = [b.ctrl && "Ctrl", b.shift && "Shift", b.alt && "Alt"].filter(Boolean);
    const base = mods.length
      ? `${mods.join(" + ")} + ${mouseButtonLabel(b.button)}`
      : mouseButtonLabel(b.button);
    return `${base} + 空格`;
  }
  const mods = [
    b.space && "空格",
    b.ctrl && "Ctrl",
    b.shift && "Shift",
    b.alt && "Alt",
  ].filter(Boolean);
  const name = mouseButtonLabel(b.button);
  return mods.length ? `${mods.join(" + ")} + ${name}` : name;
}

export default function SettingsView({ notify, refreshKey }: SettingsViewProps) {
  const [settings, setSettings] = useState<AppSettings>({
    organizeShortcuts: false,
    autoStart: false,
    realTimeOrganize: false,
    snapEnabled: true,
    defaultIconSize: 48,
    boxBgColor: "#ffffff",
    boxBgOpacity: 0,
    boxTitleColor: "#1a9c5d",
    keepOnShowDesktop: true,
    hideFileExtensions: false,
    fullDiskIndex: false,
    indexDrives: [],
    searchSort: "mtime",
    uiZoom: 1,
    mouseHotkeysEnabled: false,
    mouseHotkeys: [],
    radialBtnSize: 54,
    radialExpandSize: 440,
    radialItemSize: 66,
    radialRingGap: 28,
    radialTitle: "",
    radialIcon: "",
  });
  const [tab, setTab] = useState<TabKey>("basic");

  // 快捷方式小箭头（系统级设置，直接读注册表状态）
  const [arrowHidden, setArrowHidden] = useState(false);
  const [arrowBusy, setArrowBusy] = useState(false);

  // 组件：便签
  const [notesEnabled, setNotesEnabledState] = useState(false);
  const [notesBusy, setNotesBusy] = useState(false);

  // 组件：股票面板
  const [stocksEnabled, setStocksEnabledState] = useState(false);
  const [stocksBusy, setStocksBusy] = useState(false);

  // 组件：黄历
  const [huangliEnabled, setHuangliEnabledState] = useState(false);
  const [huangliBusy, setHuangliBusy] = useState(false);

  // 组件：毛泽东诗词
  const [poetryEnabled, setPoetryEnabledState] = useState(false);
  const [poetryBusy, setPoetryBusy] = useState(false);

  // 组件：黄帝内经
  const [huangdiEnabled, setHuangdiEnabledState] = useState(false);
  const [huangdiBusy, setHuangdiBusy] = useState(false);

  // 组件：伤寒论
  const [shanghanEnabled, setShanghanEnabledState] = useState(false);
  const [shanghanBusy, setShanghanBusy] = useState(false);
  const [rebuildBusy, setRebuildBusy] = useState(false);
  // 可读盘符列表（供「索引盘符」勾选）
  const [drives, setDrives] = useState<string[]>([]);

  const loadAll = useCallback(async () => {
    try {
      const [cfg, arrow, notesOn, stocksOn, huangliOn, poetryOn, huangdiOn, shanghanOn] = await Promise.all([
        getSettings(),
        getShortcutArrowHidden().catch(() => false),
        getNotesEnabled().catch(() => false),
        getStocksEnabled().catch(() => false),
        getHuangliEnabled().catch(() => false),
        getPoetryEnabled().catch(() => false),
        getHuangdiEnabled().catch(() => false),
        getShanghanEnabled().catch(() => false),
      ]);
      setSettings(cfg);
      setArrowHidden(arrow);
      setNotesEnabledState(notesOn);
      setStocksEnabledState(stocksOn);
      setHuangliEnabledState(huangliOn);
      setPoetryEnabledState(poetryOn);
      setHuangdiEnabledState(huangdiOn);
      setShanghanEnabledState(shanghanOn);
      listDrives().then(setDrives).catch(() => {});
    } catch (e) {
      notify(`加载设置失败：${String(e)}`, "err");
    }
  }, [notify]);

  useEffect(() => {
    loadAll();
  }, [loadAll, refreshKey]);

  // 便签 / 股票 / 黄历 / 诗词 / 黄帝内经 窗口被直接关掉时，同步勾选框状态
  useEffect(() => {
    const un1 = listen<boolean>("notes-enabled-changed", (e) => setNotesEnabledState(e.payload));
    const un2 = listen<boolean>("stocks-enabled-changed", (e) => setStocksEnabledState(e.payload));
    const un3 = listen<boolean>("huangli-enabled-changed", (e) =>
      setHuangliEnabledState(e.payload)
    );
    const un4 = listen<boolean>("poetry-enabled-changed", (e) =>
      setPoetryEnabledState(e.payload)
    );
    const un5 = listen<boolean>("huangdi-enabled-changed", (e) =>
      setHuangdiEnabledState(e.payload)
    );
    const un6 = listen<boolean>("shanghan-enabled-changed", (e) =>
      setShanghanEnabledState(e.payload)
    );
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
      un4.then((f) => f());
      un5.then((f) => f());
      un6.then((f) => f());
    };
  }, []);

  // ---------- 鼠标快捷键：识别信号与映射表编辑 ----------
  // 正在「按一下识别」的绑定 id（null = 不在识别中）
  const [capturingId, setCapturingId] = useState<number | null>(null);
  // 识别剩余秒数（横幅倒计时）
  const [captureCountdown, setCaptureCountdown] = useState(0);
  const capturingRef = useRef<number | null>(null);
  const captureTimerRef = useRef<number | null>(null);
  const captureTickRef = useRef<number | null>(null);

  const endCaptureUi = useCallback(() => {
    capturingRef.current = null;
    setCapturingId(null);
    setCaptureCountdown(0);
    if (captureTimerRef.current != null) {
      window.clearTimeout(captureTimerRef.current);
      captureTimerRef.current = null;
    }
    if (captureTickRef.current != null) {
      window.clearInterval(captureTickRef.current);
      captureTickRef.current = null;
    }
  }, []);

  // 后端回传识别到的按键 → 填进对应绑定行；Esc 取消
  useEffect(() => {
    const un1 = listen<{
      button: string;
      ctrl: boolean;
      shift: boolean;
      alt: boolean;
      space: boolean;
    }>("mouse-captured", (e) => {
      const id = capturingRef.current;
      endCaptureUi();
      if (id == null) return;
      setSettings((prev) => {
        const next = {
          ...prev,
          mouseHotkeys: (prev.mouseHotkeys ?? []).map((b) =>
            b.id === id ? { ...b, ...e.payload } : b
          ),
        };
        saveSettings(next).catch(() => {});
        const mods = [
          e.payload.space && "空格",
          e.payload.ctrl && "Ctrl",
          e.payload.shift && "Shift",
          e.payload.alt && "Alt",
        ]
          .filter(Boolean)
          .join(" + ");
        notify(
          `已识别：${mods ? mods + " + " : ""}${mouseButtonLabel(e.payload.button)}，设置已保存`,
          "ok"
        );
        return next;
      });
    });
    const un2 = listen("mouse-capture-cancel", () => {
      endCaptureUi();
      notify("已取消识别", "");
    });
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, [endCaptureUi, notify]);

  const beginCapture = useCallback(
    (id: number) => {
      capturingRef.current = id;
      setCapturingId(id);
      setCaptureCountdown(6);
      // 倒计时显示（后端 6 秒超时）
      if (captureTickRef.current != null) window.clearInterval(captureTickRef.current);
      captureTickRef.current = window.setInterval(() => {
        setCaptureCountdown((c) => {
          if (c <= 1) {
            if (captureTickRef.current != null) {
              window.clearInterval(captureTickRef.current);
              captureTickRef.current = null;
            }
            return 0;
          }
          return c - 1;
        });
      }, 1000);
      startMouseCapture().catch((e) => {
        endCaptureUi();
        notify(`无法进入识别模式：${String(e)}（请用最新构建并重启程序）`, "err");
      });
      // 兜底：后端 6 秒超时也会发 cancel 事件，这里再兜一层
      if (captureTimerRef.current != null) window.clearTimeout(captureTimerRef.current);
      captureTimerRef.current = window.setTimeout(endCaptureUi, 6800);
    },
    [endCaptureUi, notify]
  );

  const doSaveSettings = useCallback(
    async (next: AppSettings) => {
      setSettings(next);
      try {
        await saveSettings(next);
        notify("设置已保存", "ok");
      } catch (e) {
        notify(`保存设置失败：${String(e)}`, "err");
      }
    },
    [notify]
  );

  const patchBinding = useCallback(
    (id: number, patch: Partial<MouseBinding>) => {
      doSaveSettings({
        ...settings,
        mouseHotkeys: (settings.mouseHotkeys ?? []).map((b) =>
          b.id === id ? { ...b, ...patch } : b
        ),
      });
    },
    [settings, doSaveSettings]
  );

  const addBinding = useCallback(() => {
    const nb: MouseBinding = {
      id: Date.now(),
      button: "x1",
      ctrl: false,
      shift: false,
      alt: false,
      space: false,
      action: "selectAll",
      enabled: true,
    };
    doSaveSettings({ ...settings, mouseHotkeys: [...(settings.mouseHotkeys ?? []), nb] });
    // 添加即进入识别模式：直接按一下鼠标键就能绑上
    beginCapture(nb.id);
  }, [settings, doSaveSettings, beginCapture]);

  const removeBinding = useCallback(
    (id: number) => {
      doSaveSettings({
        ...settings,
        mouseHotkeys: (settings.mouseHotkeys ?? []).filter((b) => b.id !== id),
      });
    },
    [settings, doSaveSettings]
  );

  // 重复按键组合检测（同一组合绑定多次时只有第一条生效）
  const bindings = settings.mouseHotkeys ?? [];
  const hasConflict =
    settings.mouseHotkeysEnabled &&
    bindings.some(
      (b, i) =>
        b.enabled &&
        bindings.findIndex(
          (x) =>
            x.enabled &&
            x.button === b.button &&
            x.ctrl === b.ctrl &&
            x.shift === b.shift &&
            x.alt === b.alt &&
            x.space === b.space
        ) !== i
    );

  // 连续型控件（颜色选择器 / 透明度滑块）专用：先本地即时反馈，停顿 260ms 再落库。
  // 拖动时 change 事件每秒能触发几十次，直接保存会造成 IPC 风暴。
  const pendingRef = useRef<AppSettings | null>(null);
  const saveTimerRef = useRef<number | null>(null);

  const flushSettings = useCallback(async () => {
    const next = pendingRef.current;
    pendingRef.current = null;
    if (!next) return;
    try {
      await saveSettings(next);
      notify("设置已保存", "ok");
    } catch (e) {
      notify(`保存设置失败：${String(e)}`, "err");
    }
  }, [notify]);

  const doLiveSettings = useCallback(
    (next: AppSettings) => {
      setSettings(next);
      pendingRef.current = next;
      if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
      saveTimerRef.current = window.setTimeout(() => {
        saveTimerRef.current = null;
        flushSettings();
      }, 260);
    },
    [flushSettings]
  );

  // 关掉设置窗口前把还没落库的改动补上
  useEffect(() => {
    return () => {
      if (saveTimerRef.current != null) window.clearTimeout(saveTimerRef.current);
      const next = pendingRef.current;
      if (next) saveSettings(next).catch(() => {});
    };
  }, []);

  const doToggleArrow = useCallback(
    async (next: boolean) => {
      setArrowBusy(true);
      try {
        await setShortcutArrowHidden(next);
        setArrowHidden(next);
        notify(
          next
            ? "已去掉快捷方式小箭头；若个别图标还是旧的，点下面「强制生效」"
            : "已恢复快捷方式小箭头（重启了一次资源管理器，桌面会闪一下）",
          "ok"
        );
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setArrowHidden(await getShortcutArrowHidden());
        } catch {
          /* 读取失败则保留原状态 */
        }
      } finally {
        setArrowBusy(false);
      }
    },
    [notify]
  );

  // 强制生效：系统级注册表 + 重建图标缓存 + 重启资源管理器
  const doForceApplyArrow = useCallback(async () => {
    setArrowBusy(true);
    try {
      const msg = await applyShortcutArrowForced(arrowHidden);
      setArrowHidden(await getShortcutArrowHidden().catch(() => arrowHidden));
      notify(msg, "ok");
    } catch (e) {
      notify(`操作失败：${String(e)}`, "err");
    } finally {
      setArrowBusy(false);
    }
  }, [arrowHidden, notify]);

  // 组件：便签显示开关
  const doToggleNotes = useCallback(
    async (next: boolean) => {
      setNotesBusy(true);
      try {
        await setNotesEnabled(next);
        setNotesEnabledState(next);
        notify(next ? "便签已显示在桌面" : "便签已隐藏（内容仍保留）", "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setNotesEnabledState(await getNotesEnabled());
        } catch {
          /* 忽略 */
        }
      } finally {
        setNotesBusy(false);
      }
    },
    [notify]
  );

  // 组件：股票面板显示开关
  const doToggleStocks = useCallback(
    async (next: boolean) => {
      setStocksBusy(true);
      try {
        await setStocksEnabled(next);
        setStocksEnabledState(next);
        notify(next ? "股票面板已显示在桌面" : "股票面板已隐藏（自选股仍保留）", "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setStocksEnabledState(await getStocksEnabled());
        } catch {
          /* 忽略 */
        }
      } finally {
        setStocksBusy(false);
      }
    },
    [notify]
  );

  // 组件：黄历显示开关
  const doToggleHuangli = useCallback(
    async (next: boolean) => {
      setHuangliBusy(true);
      try {
        await setHuangliEnabled(next);
        setHuangliEnabledState(next);
        notify(next ? "黄历已显示在桌面" : "黄历已隐藏", "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setHuangliEnabledState(await getHuangliEnabled());
        } catch {
          /* 忽略 */
        }
      } finally {
        setHuangliBusy(false);
      }
    },
    [notify]
  );

  // 组件：诗词面板显示开关
  const doTogglePoetry = useCallback(
    async (next: boolean) => {
      setPoetryBusy(true);
      try {
        await setPoetryEnabled(next);
        setPoetryEnabledState(next);
        notify(next ? "诗词面板已显示在桌面" : "诗词面板已隐藏", "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setPoetryEnabledState(await getPoetryEnabled());
        } catch {
          /* 忽略 */
        }
      } finally {
        setPoetryBusy(false);
      }
    },
    [notify]
  );

  // 组件：黄帝内经面板显示开关
  const doToggleHuangdi = useCallback(
    async (next: boolean) => {
      setHuangdiBusy(true);
      try {
        await setHuangdiEnabled(next);
        setHuangdiEnabledState(next);
        notify(next ? "黄帝内经面板已显示在桌面" : "黄帝内经面板已隐藏", "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setHuangdiEnabledState(await getHuangdiEnabled());
        } catch {
          /* 忽略 */
        }
      } finally {
        setHuangdiBusy(false);
      }
    },
    [notify]
  );

  // 组件：伤寒论面板显示开关
  const doToggleShanghan = useCallback(
    async (next: boolean) => {
      setShanghanBusy(true);
      try {
        await setShanghanEnabled(next);
        setShanghanEnabledState(next);
        notify(next ? "伤寒论面板已显示在桌面" : "伤寒论面板已隐藏", "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
        try {
          setShanghanEnabledState(await getShanghanEnabled());
        } catch {
          /* 忽略 */
        }
      } finally {
        setShanghanBusy(false);
      }
    },
    [notify]
  );

  // 搜索：全盘索引开关
  const doToggleFullDisk = useCallback(
    async (next: boolean) => {
      const nextSettings = { ...settings, fullDiskIndex: next };
      setSettings(nextSettings);
      try {
        await saveSettings(nextSettings);
        notify(
          next ? "已开启全盘索引，正在后台重建…" : "已关闭，正在重建常用目录索引…",
          "ok"
        );
      } catch (e) {
        notify(`保存失败：${String(e)}`, "err");
        return;
      }
      setRebuildBusy(true);
      try {
        await rebuildSearchIndex(next);
        notify(
          next ? "已在后台开始全盘重建（较慢，请稍候自动生效）" : "已在后台开始重建常用目录索引",
          "ok"
        );
      } catch (e) {
        notify(`重建失败：${String(e)}`, "err");
      } finally {
        setRebuildBusy(false);
      }
    },
    [settings, notify]
  );

  // 搜索：手动重建索引
  const doRebuildIndex = useCallback(async () => {
    setRebuildBusy(true);
    notify("正在重建搜索索引…", "");
    try {
      await rebuildSearchIndex(settings.fullDiskIndex);
      notify("已在后台开始重建搜索索引（全盘较慢请稍候）", "ok");
    } catch (e) {
      notify(`重建失败：${String(e)}`, "err");
    } finally {
      setRebuildBusy(false);
    }
  }, [settings.fullDiskIndex, notify]);

  // 索引盘符：勾选立即后台建索引；取消勾选清除该盘索引
  const toggleIndexDrive = useCallback(
    async (letter: string) => {
      const on = settings.indexDrives.includes(letter);
      const next = on
        ? settings.indexDrives.filter((x) => x !== letter)
        : [...settings.indexDrives, letter];
      const nextSettings = { ...settings, indexDrives: next };
      setSettings(nextSettings);
      try {
        await saveSettings(nextSettings);
      } catch (e) {
        notify(`保存设置失败：${String(e)}`, "err");
        return;
      }
      setRebuildBusy(true);
      notify(
        on ? `正在清除盘 ${letter}…` : `正在索引盘 ${letter}（首次较慢）…`,
        ""
      );
      try {
        if (on) await clearIndex(letter);
        else await rebuildSearchIndex(undefined, letter);
        notify(on ? `已清除盘 ${letter}` : `盘 ${letter} 索引完成`, "ok");
      } catch (e) {
        notify(`操作失败：${String(e)}`, "err");
      } finally {
        setRebuildBusy(false);
      }
    },
    [settings, notify]
  );

  const doBackupLayout = useCallback(async () => {
    try {
      const path = await saveDialog({
        title: "保存桌面布局",
        defaultPath: `qidomin-layout-${new Date().toISOString().slice(0, 10)}.json`,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!path) return;
      await exportLayout(path);
      notify(`布局已保存到 ${path}`, "ok");
    } catch (e) {
      notify(`保存失败：${String(e)}`, "err");
    }
  }, [notify]);

  const doRestoreLayout = useCallback(async () => {
    try {
      const path = await openDialog({
        title: "选择布局备份文件",
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (typeof path !== "string" || !path) return;
      await importLayout(path);
      notify("布局已恢复，刷新页面…", "ok");
      setTimeout(() => window.location.reload(), 1000);
    } catch (e) {
      notify(`恢复失败：${String(e)}`, "err");
    }
  }, [notify]);

  return (
    <div className="settings-body">
      {/* ===== 左侧：类目导航 ===== */}
      <nav className="settings-nav">
        {TABS.map((t) => (
          <button
            key={t.key}
            className={`settings-nav-item${tab === t.key ? " active" : ""}`}
            onClick={() => setTab(t.key)}
            title={t.hint}
          >
            {t.label}
          </button>
        ))}
        {/* 布局备份常驻导航栏底部：不管在哪个页签都伸手可及（重装系统前找得到才叫备份） */}
        <div className="settings-nav-backup">
          <button
            className="btn"
            onClick={doBackupLayout}
            title="保存为 JSON 文件，包含分区、窗口位置/大小/外观与全局设置"
          >
            💾 保存布局
          </button>
          <button
            className="btn"
            onClick={doRestoreLayout}
            title="从 JSON 备份恢复分区布局与设置（重装系统后一键还原）"
          >
            📂 恢复布局
          </button>
        </div>
        <div className="settings-nav-foot">{APP_VERSION}</div>
      </nav>

      {/* ===== 右侧：内容区 ===== */}
      <div className="settings-wrap">
      {tab === "basic" && (
      <>
      {/* ===== 基本设置（页签名已说明内容，不再重复渲染标题） ===== */}
      <table className="data-table">
        <tbody>
          <tr>
            <td style={{ width: "40%" }}>移动吸附</td>
            <td>
              <span className="radio-row">
                <label className="radio-item">
                  <input
                    type="radio"
                    name="snapEnabled"
                    checked={settings.snapEnabled}
                    onChange={() => doSaveSettings({ ...settings, snapEnabled: true })}
                  />
                  开
                </label>
                <label className="radio-item">
                  <input
                    type="radio"
                    name="snapEnabled"
                    checked={!settings.snapEnabled}
                    onChange={() => doSaveSettings({ ...settings, snapEnabled: false })}
                  />
                  关
                </label>
              </span>
            </td>
          </tr>
          <tr>
            <td>新分区默认背景色</td>
            <td>
              <input
                type="color"
                className="color-input"
                value={settings.boxBgColor}
                onChange={(e) =>
                  doLiveSettings({ ...settings, boxBgColor: e.target.value })
                }
              />
            </td>
          </tr>
          <tr>
            <td>新分区默认透明度</td>
            <td>
              <input
                type="range"
                min={0}
                max={100}
                value={settings.boxBgOpacity}
                onChange={(e) =>
                  doLiveSettings({ ...settings, boxBgOpacity: Number(e.target.value) })
                }
                style={{ verticalAlign: "middle", marginRight: 8 }}
              />
              {settings.boxBgOpacity}%
            </td>
          </tr>
          <tr>
            <td>分区标题字体颜色</td>
            <td>
              <input
                type="color"
                className="color-input"
                value={settings.boxTitleColor}
                onChange={(e) =>
                  doLiveSettings({ ...settings, boxTitleColor: e.target.value })
                }
              />
              <span style={{ fontSize: 12, color: "var(--text-sub)", marginLeft: 8 }}>
                {settings.boxTitleColor}
              </span>
            </td>
          </tr>
          <tr>
            <td>整理时移动快捷方式（.lnk）</td>
            <td>
              <span className="radio-row">
                <label className="radio-item">
                  <input
                    type="radio"
                    name="organizeShortcuts"
                    checked={settings.organizeShortcuts}
                    onChange={() => doSaveSettings({ ...settings, organizeShortcuts: true })}
                  />
                  开
                </label>
                <label className="radio-item">
                  <input
                    type="radio"
                    name="organizeShortcuts"
                    checked={!settings.organizeShortcuts}
                    onChange={() => doSaveSettings({ ...settings, organizeShortcuts: false })}
                  />
                  关
                </label>
              </span>
            </td>
          </tr>
          <tr>
            <td>
              撤销上次整理
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                把最近一次「一键整理」移动的文件按记录搬回原位
              </div>
            </td>
            <td>
              <UndoOrganizeRow notify={notify} />
            </td>
          </tr>
          <tr>
            <td>
              去掉图标小箭头
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                失效时点强制生效
                <button
                  className="link-btn"
                  style={{ marginLeft: 6 }}
                  onClick={doForceApplyArrow}
                  title="个别位置仍是旧箭头时用：清图标缓存并重启资源管理器（桌面会闪一下）"
                >
                  仍未生效？点这里强制生效
                </button>
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={arrowHidden}
                  disabled={arrowBusy}
                  onChange={(e) => doToggleArrow(e.target.checked)}
                />
                {arrowBusy ? "处理中…" : arrowHidden ? "已去掉" : "未去掉"}
              </label>
            </td>
          </tr>
          <tr>
            <td>开机自动启动</td>
            <td>
              <span className="radio-row">
                <label className="radio-item">
                  <input
                    type="radio"
                    name="autoStart"
                    checked={settings.autoStart}
                    onChange={() => doSaveSettings({ ...settings, autoStart: true })}
                  />
                  开
                </label>
                <label className="radio-item">
                  <input
                    type="radio"
                    name="autoStart"
                    checked={!settings.autoStart}
                    onChange={() => doSaveSettings({ ...settings, autoStart: false })}
                  />
                  关
                </label>
              </span>
            </td>
          </tr>
          <tr>
            <td>
              界面缩放
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
               分区缩放和系统缩放，两者可叠加
              </div>
            </td>
            <td>
              <span className="radio-row">
                {[1, 1.25, 1.5, 1.75].map((z) => (
                  <label key={z} className="radio-item">
                    <input
                      type="radio"
                      name="uiZoom"
                      checked={Math.abs((settings.uiZoom ?? 1) - z) < 0.01}
                      onChange={() => doSaveSettings({ ...settings, uiZoom: z })}
                    />
                    {Math.round(z * 100)}%
                  </label>
                ))}
              </span>
            </td>
          </tr>
          <tr>
            <td>
              分区文件不显示后缀名
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                隐藏文件后缀名
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={!!settings.hideFileExtensions}
                  onChange={(e) => doSaveSettings({ ...settings, hideFileExtensions: e.target.checked })}
                />
                {settings.hideFileExtensions ? "已开启" : "已关闭"}
              </label>
            </td>
          </tr>
          <tr>
            <td>
              显示桌面时保持组件和分区可见
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                保持始终可见
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={!!settings.keepOnShowDesktop}
                  onChange={(e) => doSaveSettings({ ...settings, keepOnShowDesktop: e.target.checked })}
                />
                {settings.keepOnShowDesktop ? "已开启" : "已关闭"}
                </label>
                </td>
                </tr>
                </tbody>
      </table>
      </>
      )}

      {tab === "radial" && (
      <>
      <table className="data-table">
        <tbody>
          <tr>
            <td>圆钮尺寸</td>
            <td>
              <input
                type="range"
                min={40}
                max={96}
                step={2}
                value={settings.radialBtnSize}
                onChange={(e) =>
                  doLiveSettings({ ...settings, radialBtnSize: Number(e.target.value) })
                }
                style={{ verticalAlign: "middle", marginRight: 8, width: 180 }}
              />
              {settings.radialBtnSize} px
              <span style={{ fontSize: 12, color: "var(--text-sub)", marginLeft: 8 }}>
                桌面上圆钮的直径，改动实时生效
              </span>
            </td>
          </tr>
          <tr>
            <td>文件尺寸</td>
            <td>
              <input
                type="range"
                min={44}
                max={110}
                step={2}
                value={settings.radialItemSize}
                onChange={(e) =>
                  doLiveSettings({ ...settings, radialItemSize: Number(e.target.value) })
                }
                style={{ verticalAlign: "middle", marginRight: 8, width: 180 }}
              />
              {settings.radialItemSize} px
              <span style={{ fontSize: 12, color: "var(--text-sub)", marginLeft: 8 }}>
                弹出的气泡/方形卡片的边长
              </span>
            </td>
          </tr>
          <tr>
            <td>文件间距</td>
            <td>
              <input
                type="range"
                min={10}
                max={120}
                step={2}
                value={settings.radialRingGap}
                onChange={(e) =>
                  doLiveSettings({ ...settings, radialRingGap: Number(e.target.value) })
                }
                style={{ verticalAlign: "middle", marginRight: 8, width: 180 }}
              />
              {settings.radialRingGap} px
              <span style={{ fontSize: 12, color: "var(--text-sub)", marginLeft: 8 }}>
                文件之间的最小间距，环半径随文件数量自动外扩
              </span>
            </td>
          </tr>
          <tr>
            <td>饼型标题</td>
            <td>
              <input
                type="text"
                className="text-input"
                value={settings.radialTitle}
                placeholder="留空 = 显示分区名"
                onChange={(e) =>
                  doLiveSettings({ ...settings, radialTitle: e.target.value })
                }
                style={{ width: 200 }}
              />
              <span style={{ fontSize: 12, color: "var(--text-sub)", marginLeft: 8 }}>
                圆钮与弹出中心显示的文字，完整显示不截断
              </span>
            </td>
          </tr>
          <tr>
            <td style={{ verticalAlign: "top", paddingTop: 14 }}>内置图标</td>
            <td>
              <div className="radial-preset-grid">
                {RADIAL_PRESET_ICONS.map((p) => {
                  const url = svgToDataUrl(p.svg);
                  const active = settings.radialIcon === url;
                  return (
                    <button
                      key={p.key}
                      className={`radial-preset-item ${active ? "active" : ""}`}
                      title={p.label}
                      onClick={() => doLiveSettings({ ...settings, radialIcon: url })}
                    >
                      <img src={url} alt={p.label} draggable={false} />
                    </button>
                  );
                })}
              </div>
              <div style={{ fontSize: 12, color: "var(--text-sub)", marginTop: 6 }}>
                这是**默认图标**：给没单独设置图标的饼型分区用；每个饼型分区可在
                右键菜单里单独设置自己的图标（分区图标优先于这里的默认）
              </div>
            </td>
          </tr>
          <tr>
            <td>自定义图标</td>
            <td>
              {settings.radialIcon ? (
                <img
                  src={settings.radialIcon}
                  alt=""
                  style={{
                    width: 28,
                    height: 28,
                    borderRadius: "50%",
                    background: "#fff",
                    objectFit: "contain",
                    verticalAlign: "middle",
                    marginRight: 8,
                  }}
                />
              ) : null}
              <label className="btn small" style={{ cursor: "pointer" }}>
                选择图片
                <input
                  type="file"
                  accept="image/*"
                  style={{ display: "none" }}
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    e.target.value = "";
                    if (!f) return;
                    const reader = new FileReader();
                    reader.onload = () => {
                      const img = new Image();
                      img.onload = () => {
                        // 压到 128×128 再存：设置 JSON 不至于被一张原图撑爆
                        const c = document.createElement("canvas");
                        c.width = 128;
                        c.height = 128;
                        const ctx = c.getContext("2d");
                        if (!ctx) return;
                        const k = Math.min(128 / img.width, 128 / img.height);
                        ctx.drawImage(
                          img,
                          (128 - img.width * k) / 2,
                          (128 - img.height * k) / 2,
                          img.width * k,
                          img.height * k
                        );
                        doLiveSettings({ ...settings, radialIcon: c.toDataURL("image/png") });
                      };
                      img.src = String(reader.result);
                    };
                    reader.readAsDataURL(f);
                  }}
                />
              </label>
              {settings.radialIcon ? (
                <button
                  className="btn small"
                  style={{ marginLeft: 8 }}
                  onClick={() => doLiveSettings({ ...settings, radialIcon: "" })}
                >
                  清除图标
                </button>
              ) : null}
            </td>
          </tr>
        </tbody>
      </table>
      </>
      )}

      {tab === "search" && (
      <>
      <table className="data-table">
        <tbody>
          <tr>
            <td style={{ width: "40%" }}>
              全盘搜索索引
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                默认只索引常用目录（桌面 / 文档 / 下载 / 图片 / 视频 / 音乐 / 开始菜单 / 分区）。
            
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={!!settings.fullDiskIndex}
                  disabled={rebuildBusy}
                  onChange={(e) => doToggleFullDisk(e.target.checked)}
                />
                {rebuildBusy ? "处理中…" : settings.fullDiskIndex ? "已开启" : "已关闭"}
              </label>
            </td>
          </tr>
          <tr>
            <td>
              搜索结果排序
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                搜索面板里结果列表的排列方式，改完立即生效
              </div>
            </td>
            <td>
              <select
                value={settings.searchSort || "mtime"}
                onChange={(e) => {
                  const next = { ...settings, searchSort: e.target.value };
                  setSettings(next);
                  saveSettings(next).catch(() => {});
                }}
                style={{
                  fontSize: 13,
                  padding: "4px 8px",
                  borderRadius: 6,
                  border: "1px solid var(--border)",
                  background: "var(--bg, #fff)",
                }}
              >
                <option value="mtime">修改时间（最新在前）</option>
                <option value="name">文件名（文件夹优先、短名优先）</option>
                <option value="size">文件大小（大在前）</option>
              </select>
            </td>
          </tr>
          <tr>
            <td>
              索引这些盘符
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                勾选后立即后台建立该盘索引（NTFS 盘由 USN Journal 实时增量维护）；
                取消勾选会清除该盘索引
              </div>
            </td>
            <td>
              <span style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
                {drives.map((d) => {
                  const on = settings.indexDrives.includes(d);
                  return (
                    <span
                      key={d}
                      onClick={() => toggleIndexDrive(d)}
                      style={{
                        fontSize: 12,
                        padding: "3px 9px",
                        borderRadius: 6,
                        cursor: "pointer",
                        userSelect: "none",
                        background: on ? "#3b82f6" : "#f1f1f1",
                        color: on ? "#fff" : "#555",
                        border: on ? "1px solid #3b82f6" : "1px solid #e2e2e2",
                      }}
                    >
                      {d}:
                    </span>
                  );
                })}
              </span>
            </td>
          </tr>
        </tbody>
      </table>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          marginBottom: 16,
          flexWrap: "wrap",
        }}
      >
        <button className="btn" onClick={doRebuildIndex} disabled={rebuildBusy}>
          {rebuildBusy ? "重建中…" : "重建搜索索引"}
        </button>
        <span style={{ fontSize: 12, color: "var(--text-sub)" }}>
          手动刷新索引；开启全盘索引后首次较慢（后台执行）。
          搜索面板里的「范围」只影响搜索哪些索引，不改变这里的索引设置
        </span>
      </div>

      {/* ===== 搜索类目（内置后缀表，只读展示） ===== */}
      <div className="section-title">搜索类目（文件类型过滤）</div>
      <div style={{ fontSize: 12, color: "var(--text-sub)", marginBottom: 12 }}>
        搜索面板第二行按下面的大类过滤文件类型，各大类的后缀为内置定义、默认全部启用，无需配置。
      </div>
      <table className="data-table">
        <tbody>
          {FILE_CATEGORIES.map((cat) => (
            <tr key={cat.key}>
              <td style={{ width: "16%" }}>{cat.label}</td>
              <td style={{ fontSize: 12, color: "var(--text-sub)", wordBreak: "break-all" }}>
                {cat.exts.map((e) => `.${e}`).join("、")}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      </>
      )}

      {tab === "widgets" && (
      <>
      <table className="data-table">
        <tbody>
          <tr>
            <td style={{ width: "40%" }}>
              便签
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                桌面便利贴：可拖动 / 缩放，左侧带序号，每行右侧可设提醒时间（有闹钟 / 无闹钟）
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={notesEnabled}
                  disabled={notesBusy}
                  onChange={(e) => doToggleNotes(e.target.checked)}
                />
                {notesBusy ? "处理中…" : notesEnabled ? "已显示在桌面" : "未显示"}
              </label>
            </td>
          </tr>
          <tr>
            <td style={{ width: "40%" }}>
              股票面板
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                自选股实时行情：点面板右上角 ＋ 或右键菜单添加代码（600000 / 00700 / AAPL），按设定间隔自动刷新
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={stocksEnabled}
                  disabled={stocksBusy}
                  onChange={(e) => doToggleStocks(e.target.checked)}
                />
                {stocksBusy ? "处理中…" : stocksEnabled ? "已显示在桌面" : "未显示"}
              </label>
            </td>
          </tr>
          <tr>
            <td style={{ width: "40%" }}>
              黄历
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                农历干支、二十四节气、宜忌、值神星宿、冲煞方位、彭祖百忌（本地推算，断网也能用）
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={huangliEnabled}
                  disabled={huangliBusy}
                  onChange={(e) => doToggleHuangli(e.target.checked)}
                />
                {huangliBusy ? "处理中…" : huangliEnabled ? "已显示在桌面" : "未显示"}
              </label>
            </td>
          </tr>
          <tr>
            <td style={{ width: "40%" }}>
              毛泽东诗词
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                内置数十首诗词与经典名句，随机展示或按标题/内容搜索；支持自动轮播，位置/外观记忆
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={poetryEnabled}
                  disabled={poetryBusy}
                  onChange={(e) => doTogglePoetry(e.target.checked)}
                />
                {poetryBusy ? "处理中…" : poetryEnabled ? "已显示在桌面" : "未显示"}
              </label>
            </td>
          </tr>
          <tr>
            <td style={{ width: "40%" }}>
              黄帝内经
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                内置素问 81 篇（含遗篇）、灵枢 81 篇等共 324 篇全本，随机展示经文或按篇名/内容搜索；支持自动轮播，位置/外观记忆
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={huangdiEnabled}
                  disabled={huangdiBusy}
                  onChange={(e) => doToggleHuangdi(e.target.checked)}
                />
                {huangdiBusy ? "处理中…" : huangdiEnabled ? "已显示在桌面" : "未显示"}
              </label>
            </td>
          </tr>
          <tr>
            <td style={{ width: "40%" }}>
              伤寒论
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                内置宋本《伤寒论》398 条全本（太阳 / 阳明 / 少阳 / 太阴 / 少阴 / 厥阴 / 霍乱 / 劳复），随机展示条文或按方证/内容搜索；支持自动轮播，位置/外观记忆
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={shanghanEnabled}
                  disabled={shanghanBusy}
                  onChange={(e) => doToggleShanghan(e.target.checked)}
                />
                {shanghanBusy ? "处理中…" : shanghanEnabled ? "已显示在桌面" : "未显示"}
              </label>
            </td>
          </tr>
        </tbody>
      </table>
      <div style={{ fontSize: 12, color: "var(--text-sub)", margin: "6px 0 16px 4px" }}>
        组件的窗口都在桌面上，位置/大小/背景会自动记住；更多组件会陆续加到这里。
      </div>
      </>
      )}

      {tab === "hotkeys" && (
      <>
      {/* 识别中横幅：全页可见，带倒计时 */}
      {capturingId != null && (
        <div
          style={{
            position: "sticky",
            top: 0,
            zIndex: 10,
            background: "#fff7e6",
            border: "1px solid #f59e0b",
            borderRadius: 8,
            padding: "8px 12px",
            marginBottom: 10,
            fontSize: 13,
            color: "#92400e",
            fontWeight: 600,
          }}
        >
          正在识别（剩 {captureCountdown} 秒）：按一下鼠标中键 / 侧键（连按两下中键 =
          「双击中键」），按 Esc 取消。识别成功后按键会被自动填入绑定行。
        </div>
      )}
      {/* ===== 鼠标快捷键 ===== */}
      <table className="data-table">
        <tbody>
          <tr>
            <td style={{ width: "40%" }}>
              启用鼠标快捷键
              <div style={{ fontSize: 12, color: "var(--text-sub)" }}>
                开启后下表中启用的按键会被全局接管，其他程序收不到——
                例如接管「侧键2（后退）」后，浏览器侧键后退会失效，知情后自行取舍
              </div>
            </td>
            <td>
              <label className="switch-cell">
                <input
                  type="checkbox"
                  checked={!!settings.mouseHotkeysEnabled}
                  onChange={(e) =>
                    doSaveSettings({ ...settings, mouseHotkeysEnabled: e.target.checked })
                  }
                />
                {settings.mouseHotkeysEnabled ? "已开启" : "已关闭"}
              </label>
            </td>
          </tr>
        </tbody>
      </table>

      <div className="section-title">映射表（按一下鼠标识别按键）</div>
      {bindings.length === 0 && (
        <div style={{ fontSize: 12, color: "var(--text-sub)", margin: "0 0 10px 4px" }}>
          还没有绑定。点下方「添加绑定」，然后直接按一下鼠标侧键 / 中键即可绑上。
        </div>
      )}
      <table className="data-table mouse-map-table">
        <thead>
          <tr>
            <th style={{ width: "20%" }}>鼠标按键</th>
            <th style={{ width: "20%" }}>触发方式</th>
            <th>执行动作</th>
            <th style={{ width: "11%" }}>状态</th>
            <th style={{ width: "9%", textAlign: "right" }}>操作</th>
          </tr>
        </thead>
        <tbody>
          {bindings.map((b) => (
            <tr key={b.id}>
              <td>
                {capturingId === b.id ? (
                  <button className="btn capturing" title="正在等待鼠标按键…">
                    按鼠标键…（{captureCountdown}s）
                  </button>
                ) : (
                  <button
                    className="btn"
                    onClick={() => beginCapture(b.id)}
                    title="点击后按一下鼠标按键，自动识别填入"
                  >
                    {bindingLabel(b)}
                  </button>
                )}
              </td>
              <td>
                <select
                  value={b.mode === "spaceAfter" ? "spaceAfter" : "press"}
                  onChange={(e) => patchBinding(b.id, { mode: e.target.value })}
                  title="「鼠标键 + 空格」组合：两个键同时按着就触发，不分先后（先按哪个都行；松开其中一个就不算了）。鼠标键先按时会放行给当前软件（转视图等照常），空格先按时不会往当前程序里漏空格。注意：只对中键 / 侧键有效"
                >
                  <option value="press">按下即触发</option>
                  <option value="spaceAfter" disabled={!isChordableButton(b.button)}>
                    键 + 空格
                  </option>
                </select>
              </td>
              <td>
                <select
                  value={b.action}
                  onChange={(e) => patchBinding(b.id, { action: e.target.value })}
                >
                  <optgroup label="系统操作（对当前焦点窗口生效）">
                    {MOUSE_ACTIONS.filter((a) => a.combo).map((a) => (
                      <option key={a.key} value={a.key}>
                        {a.label}（{a.combo}）
                      </option>
                    ))}
                  </optgroup>
                  <optgroup label="Qidomin 功能">
                    {MOUSE_ACTIONS.filter((a) => !a.combo).map((a) => (
                      <option key={a.key} value={a.key}>
                        {a.label}
                      </option>
                    ))}
                  </optgroup>
                </select>
              </td>
              <td>
                <label className="switch-cell">
                  <input
                    type="checkbox"
                    checked={b.enabled}
                    onChange={(e) => patchBinding(b.id, { enabled: e.target.checked })}
                  />
                  {b.enabled ? "启用" : "停用"}
                </label>
              </td>
              <td style={{ textAlign: "right" }}>
                <button
                  className="link-btn"
                  style={{ color: "#ef4444" }}
                  onClick={() => removeBinding(b.id)}
                >
                  删除
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 12,
          margin: "10px 0 16px 4px",
          flexWrap: "wrap",
        }}
      >
        <button className="btn primary" onClick={addBinding}>
          添加绑定
        </button>
        {hasConflict && (
          <span style={{ fontSize: 12, color: "#ef4444" }}>
            存在重复的按键组合，只有排在前面的会生效
          </span>
        )}
        <span style={{ fontSize: 12, color: "var(--text-sub)" }}>
          左/右键不可绑定；点按钮后按一下鼠标侧键 / 中键即可自动识别
        </span>
      </div>

      </>
      )}

      {tab === "about" && (
      <>
      {/* ===== 关于：刻意极简 —— 打赏码放最前面，打开页签不用滚动就能看见 ===== */}
      <div className="about-card">
        <div className="about-head">
          <div className="about-logo">净</div>
          <div>
            <div className="about-name">净界 Qidomin</div>
            <div className="about-ver">
              {APP_VERSION} · 桌面分区整理 · 开源免费，本地运行，数据存本机
            </div>
          </div>
        </div>

        {/* 打赏：左文右码，码放白底上保证任何主题下都清晰可扫 */}
        <div className="about-donate">
          <div className="about-donate-text">
            <div className="about-donate-title">请作者喝杯咖啡 ☕</div>
            <div className="about-donate-sub">
              微信 / 支付宝扫码打赏，金额随意，心意最重。
              <br />
              打赏不会解锁任何功能——净界永远完整免费。
            </div>
          </div>
          <img
            className="about-donate-qr"
            src="/donate-qr.png"
            alt="收款打赏码"
            draggable={false}
          />
        </div>

        {/* 开源免费声明：压缩成一段，不打扰打赏区的第一屏位置 */}
        <div className="about-opensource">
          <p>
            净界是一款<b>开源免费软件</b>：无广告、不推广、不上传数据，任何人都可以自由使用与分享。
            开发维护全凭兴趣，你的每一笔打赏都是对作者最直接的支持，谢谢！
          </p>
        </div>

        <div className="about-meta">
          <span>© 2026-9-11 @桂枝汤QQ363899135-净界 Qidomin</span>
          <span>技术架构：Tauri 2 + Rust + React</span>
        </div>
      </div>
      </>
      )}
      </div>
    </div>
  );
}
