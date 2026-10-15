import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getLastOrganizeBatch,
  organizeDesktopCommit,
  organizeDesktopPlan,
  undoLastOrganize,
} from "../api";
import type { OrganizePlan, OrganizePlanItem } from "../types";

interface Props {
  notify: (text: string, kind: "ok" | "err" | "") => void;
}

function formatSize(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

/** 执行结果显示（成功 + 失败明细 + 撤销按钮） */
interface CommitState {
  moved: number;
  failed: number;
  errors: string[];
  batchId: string | null;
}

/**
 * 「一键整理」的预览对话框。
 *
 * 托盘菜单 / 鼠标热键触发 `organize-request` 事件后弹出。整理拆成两步：
 * 先 plan（只算不动，正在写入的文件会被单独列为"跳过"），用户看清每个文件
 * 要去哪、依据是什么，还能取消勾选；确认后才真正移动。
 * 搬完的结果可以一键撤销（按批次记录原路搬回）。
 */
export default function OrganizePreview({ notify }: Props) {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);
  const [plan, setPlan] = useState<OrganizePlan | null>(null);
  /** 勾选了要搬的文件（key = 源路径）。默认全选，用户可取消 */
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<CommitState | null>(null);
  const [showErrors, setShowErrors] = useState(false);
  /** 最近一次整理批次（对话框关闭重开后还能撤销） */
  const [lastBatch, setLastBatch] = useState<[string, number, string] | null>(null);

  const refreshLastBatch = useCallback(() => {
    getLastOrganizeBatch()
      .then(setLastBatch)
      .catch(() => {});
  }, []);

  useEffect(() => {
    refreshLastBatch();
  }, [refreshLastBatch]);

  const openPlan = useCallback(async () => {
    setLoading(true);
    setResult(null);
    setShowErrors(false);
    try {
      const p = await organizeDesktopPlan();
      setPlan(p);
      setSelected(new Set(p.items.map((i) => i.path)));
      setOpen(true);
    } catch (e) {
      notify(`生成整理预览失败：${String(e)}`, "err");
    } finally {
      setLoading(false);
    }
  }, [notify]);

  useEffect(() => {
    const un = listen("organize-request", () => {
      openPlan();
    });
    return () => {
      un.then((f) => f());
    };
  }, [openPlan]);

  const toggle = useCallback((path: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  // 按目标分区分组展示，组头显示"去哪、几件、依据"
  const groups = useMemo(() => {
    if (!plan) return [] as { boxName: string; items: OrganizePlanItem[] }[];
    const order: string[] = [];
    const map = new Map<string, OrganizePlanItem[]>();
    for (const it of plan.items) {
      const list = map.get(it.targetBoxName);
      if (list) list.push(it);
      else {
        map.set(it.targetBoxName, [it]);
        order.push(it.targetBoxName);
      }
    }
    return order.map((boxName) => ({ boxName, items: map.get(boxName)! }));
  }, [plan]);

  const checkedCount = selected.size;

  const doCommit = useCallback(async () => {
    if (!plan || checkedCount === 0) return;
    setRunning(true);
    try {
      const items = plan.items
        .filter((i) => selected.has(i.path))
        .map((i) => ({ path: i.path, targetBoxId: i.targetBoxId }));
      const r = await organizeDesktopCommit(items);
      setResult(r);
      setShowErrors(false);
      refreshLastBatch();
      if (r.failed > 0) {
        notify(`整理完成：${r.moved} 个成功，${r.failed} 个失败`, "err");
      } else {
        notify(`整理完成：已移动 ${r.moved} 个文件`, "ok");
      }
    } catch (e) {
      notify(`执行整理失败：${String(e)}`, "err");
    } finally {
      setRunning(false);
    }
  }, [plan, checkedCount, selected, notify, refreshLastBatch]);

  const doUndo = useCallback(async () => {
    setRunning(true);
    try {
      const [back, total] = await undoLastOrganize();
      notify(`已撤销：${back}/${total} 个文件回到原位`, back === total ? "ok" : "err");
      setResult(null);
      setLastBatch(null);
      setOpen(false);
    } catch (e) {
      notify(`撤销失败：${String(e)}`, "err");
    } finally {
      setRunning(false);
    }
  }, [notify]);

  if (!open) return null;

  const empty = plan !== null && plan.items.length === 0 && plan.busy.length === 0;

  return (
    <div className="orgp-mask" onPointerDown={(e) => e.stopPropagation()}>
      <div className="orgp-card">
        <div className="orgp-head">
          <span>一键整理预览</span>
          <button className="icon-btn" title="关闭" onClick={() => setOpen(false)}>
            ✕
          </button>
        </div>

        {empty ? (
          <div className="orgp-empty">桌面很干净，没有需要整理的文件。</div>
        ) : (
          <>
            {plan && plan.busy.length > 0 && (
              <div className="orgp-busy">
                {plan.busy.length} 个文件正在被使用（下载 / 编辑中），本轮跳过：
                {plan.busy.slice(0, 5).join("、")}
                {plan.busy.length > 5 ? "…" : ""}
              </div>
            )}

            {/* 执行结果覆盖在列表上方：只看结果，不再显示计划 */}
            {result ? (
              <div className="orgp-result">
                <div className="orgp-result-line">
                  已移动 {result.moved} 个文件
                  {result.failed > 0 ? `，${result.failed} 个失败` : ""}。
                </div>
                {result.errors.length > 0 && (
                  <>
                    <button className="link-btn" onClick={() => setShowErrors((v) => !v)}>
                      {showErrors ? "收起失败明细" : `查看失败明细（${result.errors.length}）`}
                    </button>
                    {showErrors && (
                      <ul className="orgp-errors">
                        {result.errors.map((e, i) => (
                          <li key={i}>{e}</li>
                        ))}
                      </ul>
                    )}
                  </>
                )}
                {result.batchId && (
                  <button className="btn ghost" disabled={running} onClick={doUndo}>
                    撤销本次整理（搬回原位）
                  </button>
                )}
              </div>
            ) : (
              <>
                <div className="orgp-summary">
                  共 {plan?.items.length ?? 0} 个文件将按规则归入分区，
                  <label className="orgp-select-all">
                    <input
                      type="checkbox"
                      checked={plan !== null && checkedCount === plan.items.length}
                      onChange={() => {
                        if (!plan) return;
                        if (checkedCount === plan.items.length) setSelected(new Set());
                        else setSelected(new Set(plan.items.map((i) => i.path)));
                      }}
                    />
                    全选
                  </label>
                </div>
                <div className="orgp-list">
                  {groups.map((g) => (
                    <div key={g.boxName} className="orgp-group">
                      <div className="orgp-group-head">
                        <span className="orgp-group-name">{g.boxName}</span>
                        <span className="orgp-group-count">{g.items.length} 项</span>
                      </div>
                      {g.items.map((it) => (
                        <label key={it.path} className="orgp-item" title={it.path}>
                          <input
                            type="checkbox"
                            checked={selected.has(it.path)}
                            onChange={() => toggle(it.path)}
                          />
                          <span className="orgp-item-name">{it.name}</span>
                          <span className="orgp-item-meta">
                            {formatSize(it.size)} · {it.matchedBy}
                          </span>
                        </label>
                      ))}
                    </div>
                  ))}
                </div>
                <div className="orgp-foot">
                  <span className="orgp-foot-hint">
                    {checkedCount} / {plan?.items.length ?? 0} 已勾选，移动后可撤销
                  </span>
                  <div style={{ flex: 1 }} />
                  <button className="btn ghost" disabled={running} onClick={() => setOpen(false)}>
                    取消
                  </button>
                  <button
                    className="btn"
                    disabled={running || checkedCount === 0}
                    onClick={doCommit}
                  >
                    {running ? "整理中…" : `整理 ${checkedCount} 个文件`}
                  </button>
                </div>
              </>
            )}

            {/* 上次批次还没撤销过 → 关闭后仍可在这里补救 */}
            {!result && lastBatch && (
              <div className="orgp-last">
                上次整理了 {lastBatch[1]} 个文件（{lastBatch[2]}）
                <button className="link-btn" disabled={running} onClick={doUndo}>
                  撤销
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
