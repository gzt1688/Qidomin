import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import {
  searchFiles,
  openFile,
  rebuildSearchIndex,
  readClipboard,
  listDrives,
  getSettings,
  saveSettings,
  startSearchFileDrag,
} from "../api";
import { FILE_CATEGORIES } from "../fileCategories";
import type { SearchItem, AppSettings } from "../types";

function fmtSize(n: number): string {
  if (!n) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** 修改时间：Unix 秒 → "2026-09-13 10:24"（0 = 未知，显示空） */
function fmtTime(sec: number): string {
  if (!sec) return "";
  const d = new Date(sec * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 范围/类型小圆片按钮样式：state 为 "on"（选中，品牌色）/"off"（排除，红色）/""（不参与，灰色） */
function chip(state: "on" | "off" | "", color: string): CSSProperties {
  const bg = state === "on" ? color : state === "off" ? "#ef4444" : "#f1f1f1";
  const border =
    state === "on"
      ? `1px solid ${color}`
      : state === "off"
        ? "1px solid #ef4444"
        : "1px solid #e2e2e2";
  return {
    fontSize: 11,
    lineHeight: "16px",
    padding: "1px 8px",
    borderRadius: 5,
    cursor: "pointer",
    userSelect: "none",
    background: bg,
    color: state === "" ? "#555" : "#fff",
    border,
  };
}

const CHIP_HINT = "左键：选中；右键：排除（红色）";

export default function SearchWindow() {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchItem[]>([]);
  const [selected, setSelected] = useState(0);
  const [hint, setHint] = useState("输入即搜：文件名模糊匹配，前缀优先");
  const [drives, setDrives] = useState<string[]>([]);
  const [indexDrives, setIndexDrives] = useState<string[]>([]);
  const [fullDisk, setFullDisk] = useState(false);
  const [busy, setBusy] = useState(false);
  // 搜索范围：null = 全部（常用 + 所有盘）；否则为所选 scope（"common" / 盘符）多选
  const [scope, setScope] = useState<string[] | null>(null);
  // 右键排除的范围（红芯片）。有选中项时排除项自然不在搜索集里；
  // 无选中项时 = 全部范围去掉这些排除项
  const [scopeEx, setScopeEx] = useState<string[]>([]);
  // 后台补建索引完成后 +1，触发结果重查
  const [indexTick, setIndexTick] = useState(0);
  // 当前激活的文件大类（null = 全部，不过滤类型；后缀表内置、默认全启用）
  const [activeCat, setActiveCat] = useState<string | null>(null);
  // 右键排除的文件大类（红芯片，可多选）：命中这些大类后缀的文件不参与搜索
  const [exCats, setExCats] = useState<string[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // 读取设置（类目 / 已索引盘 / 全盘开关）。搜索窗口常驻不销毁，
  // 用户可能在设置页改过类目，所以每次窗口获得焦点都要重读一遍。
  const reloadSettings = useCallback(() => {
    getSettings()
      .then((s: AppSettings) => {
        setIndexDrives(s.indexDrives || []);
        setFullDisk(!!s.fullDiskIndex);
      })
      .catch(() => {});
  }, []);

  // 初始化：可读盘符列表 + 设置
  useEffect(() => {
    listDrives().then(setDrives).catch(() => {});
    reloadSettings();
  }, [reloadSettings]);

  // 手动拖动：pointerdown 记录偏移，pointermove 实时 setPosition。
  // 系统 start_dragging 在部分环境进入模态循环前有明显延迟（表现为松手后窗口
  // 才跳过去），手动跟随完全由自己控制，最稳。芯片（role=button）不参与拖动。
  const dragRef = useRef<{ sx: number; sy: number; wx: number; wy: number } | null>(null);

  const startDrag = useCallback(async (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    const t = e.target as HTMLElement;
    if (t.closest('[role="button"]')) return;
    e.preventDefault();
    const pos = await getCurrentWindow().outerPosition();
    dragRef.current = { sx: e.screenX, sy: e.screenY, wx: pos.x, wy: pos.y };
  }, []);

  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      const d = dragRef.current;
      if (!d) return;
      getCurrentWindow()
        .setPosition(
          new LogicalPosition(
            Math.round(d.wx + (e.screenX - d.sx)),
            Math.round(d.wy + (e.screenY - d.sy))
          )
        )
        .catch(() => {});
    };
    const onUp = () => {
      dragRef.current = null;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
  }, []);

  // 获得焦点时：重读设置 + 让输入框拿到键盘焦点（不再失焦自动隐藏，避免与托盘切换冲突）
  useEffect(() => {
    const w = getCurrentWindow();
    const p = w.onFocusChanged(({ payload: focused }) => {
      if (focused) {
        reloadSettings();
        inputRef.current?.focus();
      }
    });
    return () => {
      p.then((f) => f()).catch(() => {});
    };
  }, [reloadSettings]);

  // 初次聚焦兜底
  useEffect(() => {
    const t = window.setTimeout(() => inputRef.current?.focus(), 60);
    // 首屏渲染完成：通知后端可以显示窗口了（后端等这个信号，避免首次弹出白屏）
    emit("search-window-ready").catch(() => {});
    return () => window.clearTimeout(t);
  }, []);

  // 输入即搜（带搜索范围 scopes）
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setResults([]);
      setSelected(0);
      setHint("输入即搜：文件名模糊匹配，前缀优先");
      return;
    }
    // 类型过滤：直接使用内置大类后缀表（默认全启用）
    const cat = activeCat
      ? FILE_CATEGORIES.find((c) => c.key === activeCat)
      : undefined;
    const exts = cat ? cat.exts : undefined;
    const extLabel = cat?.label ?? "";
    // 类型排除：被右键排除的大类，其全部后缀不参与搜索
    const excludeExts =
      exCats.length > 0
        ? FILE_CATEGORIES.filter((c) => exCats.includes(c.key)).flatMap(
            (c) => c.exts
          )
        : undefined;
    const t = window.setTimeout(async () => {
      try {
        // null = 全部（常用 + 所有可读盘）；已勾选的具体项 = 只搜这些 scope；
        // 右键排除项始终从搜索范围里去掉（无选中项时 = 全部范围去掉排除项）
        const scopes = (
          scope ?? ["common", ...drives]
        ).filter((s) => !scopeEx.includes(s));
        // 排除的盘符 → 路径前缀排除：C 盘的桌面/文档等在「常用」scope 里，
        // 光把 C 从范围里拿掉拦不住它们，后端要按路径前缀再滤一层
        const excludeScopes = scopeEx.filter((s) => s !== "common");
        const r = await searchFiles(
          q,
          50,
          scopes,
          exts,
          excludeExts,
          excludeScopes.length ? excludeScopes : undefined
        );
        setResults(r);
        setSelected(0);
        setHint(
          r.length
            ? `找到 ${r.length} 项`
            : exts
              ? `没有匹配的文件（${extLabel}：${exts.length} 种后缀，可在设置-搜索里调整）`
              : "没有匹配的文件"
        );
      } catch (e) {
        setHint(`搜索失败：${String(e)}`);
      }
    }, 120);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, drives, scope, scopeEx, activeCat, exCats, indexTick]);

  // 切换范围（左键 = 选中）：从「全部」点某项 = 只选它；多选可叠加；清空选择回到「全部」。
  // 点的是红色（已排除）芯片时，左键等于把它拉回选中
  const toggleScope = useCallback(
    (key: string) => {
      let next: string[] | null;
      if (scope === null) {
        next = [key];
      } else {
        const s = scope.includes(key)
          ? scope.filter((x) => x !== key)
          : [...scope, key];
        next = s.length ? s : null;
      }
      setScope(next);
      // 左键选中即取消排除
      setScopeEx((ex) => ex.filter((x) => x !== key));
      // 选中的盘若还没建索引 → 后台补建（不打断当前搜索，完成后自动重查）
      if (
        key !== "common" &&
        next?.includes(key) &&
        !fullDisk &&
        !indexDrives.includes(key)
      ) {
        const nextIdx = [...indexDrives, key];
        setIndexDrives(nextIdx);
        setBusy(true);
        setHint(`正在索引盘 ${key}（首次较慢）…`);
        (async () => {
          try {
            const cur = await getSettings();
            await saveSettings({ ...cur, indexDrives: nextIdx });
          } catch {
            /* 忽略保存失败 */
          }
          try {
            await rebuildSearchIndex(undefined, key);
            setHint(`盘 ${key} 索引完成`);
          } catch (e) {
            setHint(`索引失败：${String(e)}`);
          } finally {
            setBusy(false);
            setIndexTick((t) => t + 1);
          }
        })();
      }
    },
    [scope, fullDisk, indexDrives]
  );

  // 右键范围 = 排除/取消排除。排除项同时从选中集里移除
  const excludeScope = useCallback((key: string) => {
    setScopeEx((ex) =>
      ex.includes(key) ? ex.filter((x) => x !== key) : [...ex, key]
    );
    setScope((s) => {
      if (!s || !s.includes(key)) return s;
      const rest = s.filter((x) => x !== key);
      return rest.length ? rest : null;
    });
  }, []);

  // 类型大类：左键 = 选中（单选，再点取消）；点红色（已排除）芯片等于选中它
  const toggleCat = useCallback((key: string) => {
    setActiveCat((cur) => (cur === key ? null : key));
    setExCats((ex) => ex.filter((x) => x !== key));
  }, []);

  // 类型大类：右键 = 排除/取消排除（可多选）。排除项从选中里移除
  const excludeCat = useCallback((key: string) => {
    setExCats((ex) =>
      ex.includes(key) ? ex.filter((x) => x !== key) : [...ex, key]
    );
    setActiveCat((cur) => (cur === key ? null : cur));
  }, []);

  const open = useCallback((item: SearchItem) => {
    openFile(item.path).catch((e) => console.error("打开失败", e));
    getCurrentWindow().hide().catch(() => {});
  }, []);

  /** 拖出刚结束的标记：拖动过程会吃掉 pointerup，click 通常不会触发，
   *  但个别情况下还会补一下 —— 有这个标记就不把"拖完"当成"单击打开" */
  const dragOutRef = useRef(false);

  /**
   * 按住搜索结果拖动超过阈值 → 启动系统原生 OLE 拖拽。
   * 拖出去的是**真文件**：落桌面 = 复制出来，落微信/邮件等程序 = 直接使用，
   * 落到自己的分区 = 整理进分区。6px 阈值不影响普通"单击打开"。
   */
  const beginResultDrag = useCallback((e: ReactPointerEvent, path: string) => {
    if (e.button !== 0) return;
    const sx = e.screenX;
    const sy = e.screenY;
    let started = false;
    const finish = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
    const onMove = (ev: PointerEvent) => {
      if (started) return;
      if (Math.abs(ev.screenX - sx) < 6 && Math.abs(ev.screenY - sy) < 6) return;
      started = true;
      dragOutRef.current = true;
      // 命令在用户松开鼠标（拖放结束）后才 resolve
      startSearchFileDrag(path)
        .catch((err) => console.error("拖拽文件失败", path, err))
        .finally(() => {
          setTimeout(() => (dragOutRef.current = false), 120);
          finish();
        });
    };
    const onUp = () => finish();
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
  }, []);

  // 粘贴：只把**文字**填进搜索框（自动触发即搜）。
  //
  // 剪贴板里不是文字（复制的是文件 / 图片，或者干脆是空的）时只给一句提示，
  // 不拿文件名去搜，也不搬任何文件 —— 要放文件请直接拖到分区面板上。
  const doPaste = useCallback(async () => {
    try {
      const info = await readClipboard();
      if (info.kind === "text" && info.text.trim()) {
        setQuery(info.text.replace(/\s+/g, " ").slice(0, 100));
        inputRef.current?.focus();
        return;
      }
      // 不是文字（复制的是文件、图片，或剪贴板是空的）→ 一句话带过
      setHint("剪贴板不是文字，请复制文字后粘贴。");
    } catch {
      // 极少数：剪贴板被别的程序锁住。也只提示一句，不弹报错框
      setHint("剪贴板暂时读不到内容，稍后重试");
    }
  }, []);



  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === "Escape") {
        getCurrentWindow().hide().catch(() => {});
        return;
      }
      if (e.key === "ArrowDown") {
        setSelected((s) => Math.min(s + 1, Math.max(results.length - 1, 0)));
        e.preventDefault();
        return;
      }
      if (e.key === "ArrowUp") {
        setSelected((s) => Math.max(s - 1, 0));
        e.preventDefault();
        return;
      }
      if (e.key === "Enter") {
        const it = results[selected];
        if (it) open(it);
        e.preventDefault();
      }
    },
    [results, selected, open]
  );

  useEffect(() => {
    const el = listRef.current?.querySelector(`[data-idx="${selected}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [selected, results]);

  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        display: "flex",
        flexDirection: "column",
        background: "#ffffff",
        color: "#222",
        fontFamily: "system-ui, -apple-system, 'Microsoft YaHei', sans-serif",
      }}
      onKeyDown={onKeyDown}
      // 整个搜索面板屏蔽 WebView 默认右键菜单：右键保留给芯片「排除」操作
      onContextMenu={(e) => e.preventDefault()}
    >
      {/* 搜索范围（顶部）：手动拖动；role=button 的芯片只响应点击 */}
      <div
        onPointerDown={startDrag}
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 5,
          rowGap: 4,
          alignItems: "center",
          padding: "5px 14px",
          borderBottom: "1px solid #eee",
          cursor: "move",
        }}
      >
        <span style={{ fontSize: 12, color: "#999", marginRight: 2 }}>范围：</span>
        <span
          role="button"
          onClick={() => {
            setScope(null);
            setScopeEx([]);
          }}
          title="全部（点击重置选中与排除）"
          style={chip(scope === null && scopeEx.length === 0 ? "on" : "", "#22c55e")}
        >
          全部
        </span>
        <span
          role="button"
          onClick={() => toggleScope("common")}
          onContextMenu={(e) => {
            e.preventDefault();
            excludeScope("common");
          }}
          title={`桌面 / 文档 / 下载 / 图片 / 音乐 / 开始菜单等常用目录（${CHIP_HINT}）`}
          style={chip(
            scope?.includes("common") ? "on" : scopeEx.includes("common") ? "off" : "",
            "#3b82f6"
          )}
        >
          常用
        </span>
        {drives.map((d) => {
          const on = scope?.includes(d) ?? false;
          const off = scopeEx.includes(d);
          return (
            <span
              key={d}
              role="button"
              onClick={() => toggleScope(d)}
              onContextMenu={(e) => {
                e.preventDefault();
                excludeScope(d);
              }}
              title={`盘 ${d}:（${CHIP_HINT}）`}
              style={chip(on ? "on" : off ? "off" : "", "#3b82f6")}
            >
              {d}:
            </span>
          );
        })}
        <span
          role="button"
          onClick={() => getCurrentWindow().hide()}
          title="关闭"
          style={{
            marginLeft: "auto",
            cursor: "pointer",
            fontSize: 18,
            lineHeight: "20px",
            color: "#999",
            padding: "0 6px",
          }}
        >
          ×
        </span>
      </div>

      {/* 文件类型大类过滤（第二行）：手动拖动 */}
      <div
        onPointerDown={startDrag}
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 5,
          rowGap: 4,
          alignItems: "center",
          padding: "5px 14px",
          borderBottom: "1px solid #eee",
        }}
      >
        <span style={{ fontSize: 12, color: "#999", marginRight: 2 }}>类型：</span>
        <span
          role="button"
          onClick={() => {
            setActiveCat(null);
            setExCats([]);
          }}
          title="全部类型（点击重置选中与排除）"
          style={chip(activeCat === null && exCats.length === 0 ? "on" : "", "#22c55e")}
        >
          全部
        </span>
        {FILE_CATEGORIES.map((c) => {
          const on = activeCat === c.key;
          const off = exCats.includes(c.key);
          return (
            <span
              key={c.key}
              role="button"
              onClick={() => toggleCat(c.key)}
              onContextMenu={(e) => {
                e.preventDefault();
                excludeCat(c.key);
              }}
              title={`${c.label}（${c.exts.length} 种后缀，${CHIP_HINT}）`}
              style={chip(on ? "on" : off ? "off" : "", "#3b82f6")}
            >
              {c.label}
            </span>
          );
        })}
      </div>

      {/* 结果列表（中） */}
      <div ref={listRef} style={{ flex: 1, overflowY: "auto", padding: "6px 6px" }}>
        {results.map((it, i) => (
          <div
            key={it.path}
            data-idx={i}
            onMouseEnter={() => setSelected(i)}
            onPointerDown={(e) => beginResultDrag(e, it.path)}
            onClick={() => {
              if (dragOutRef.current) return;
              open(it);
            }}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 10,
              padding: "4px 10px",
              borderRadius: 6,
              cursor: "pointer",
              background: i === selected ? "#eef5ff" : "transparent",
              userSelect: "none",
            }}
          >
            <div style={{ minWidth: 0, flex: 1 }}>
              <div
                style={{
                  fontSize: 13,
                  color: "#222",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
              >
                {it.name}
                {it.isDir ? "/" : ""}
              </div>
              <div
                style={{
                  fontSize: 11,
                  color: "#999",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
              >
                {it.path}
              </div>
            </div>
            {/* 右列与左侧同构：上=文件大小，下=修改日期 */}
            <div
              style={{
                flexShrink: 0,
                textAlign: "right",
                fontSize: 11,
                lineHeight: "15px",
                color: "#aaa",
                fontVariantNumeric: "tabular-nums",
              }}
            >
              <div>{it.isDir ? "文件夹" : fmtSize(it.size)}</div>
              <div>{fmtTime(it.mtime)}</div>
            </div>
          </div>
        ))}
      </div>

      {/* 搜索框（底部）：粘贴按钮绿底白字通栏占满 + 输入框 */}
      <div
        style={{
          display: "flex",
          alignItems: "stretch",
          borderTop: "1px solid #eee",
          background: "#fff",
        }}
      >
        <button
          onClick={doPaste}
          title="读取剪贴板内容并立即搜索"
          style={{
            flexShrink: 0,
            fontSize: 13,
            padding: "0 22px",
            border: "none",
            background: "#22c55e",
            color: "#fff",
            cursor: "pointer",
          }}
        >
          粘贴
        </button>
        <input
          ref={inputRef}
          style={{
            flex: 1,
            minWidth: 0,
            border: "none",
            outline: "none",
            padding: "13px 12px",
            fontSize: 17,
            background: "#fff",
            color: "#222",
          }}
          placeholder="搜索文件 / 应用…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      <div
        style={{
          borderTop: "1px solid #f0f0f0",
          padding: "2px 14px 5px",
          fontSize: 12,
          lineHeight: "18px",
          color: "#999",
        }}
      >
        {hint}
      </div>
    </div>
  );
}
