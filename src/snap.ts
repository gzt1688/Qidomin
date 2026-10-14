/**
 * 窗口磁性吸附（分区 / 便签 / 股票 / 黄历 / 诗词共用）。
 *
 * 候选矩形由后端 `get_box_snap_targets` 提供：其它可见分区 + 其它可见组件 +
 * 自己所在屏幕的工作区，全部物理像素。阈值沿用设置里的 snapThreshold（默认 20）。
 */
import { invoke } from "@tauri-apps/api/core";

export type SnapRect = [number, number, number, number];

export interface SnapData {
  rects: SnapRect[];
  threshold: number;
}

export function fetchSnapTargets(owner: string): Promise<SnapData> {
  return invoke("get_box_snap_targets", { owner });
}

const EMPTY: SnapData = { rects: [], threshold: 0 };

/** 失败不抛：吸附只是体验增强，拿不到候选就当没有 */
export function fetchSnapQuietly(owner: string): Promise<SnapData> {
  return fetchSnapTargets(owner).catch(() => EMPTY);
}

/**
 * 拖动磁性吸附：窗口四条边各自找阈值内最近的候选边，x/y 两轴独立贴齐。
 * 返回贴齐后的左上角坐标（两轴都没命中则原样返回）。
 */
export function magnetSnap(
  nx: number,
  ny: number,
  w: number,
  h: number,
  targets: SnapRect[],
  threshold: number
): [number, number] {
  if (threshold <= 0 || targets.length === 0) return [nx, ny];
  let bx: number | null = null;
  let bd = threshold;
  let by: number | null = null;
  let bdY = threshold;
  for (const r of targets) {
    for (const e of [r[0], r[0] + r[2]]) {
      let d = Math.abs(nx - e);
      if (d < bd) {
        bd = d;
        bx = e;
      }
      d = Math.abs(nx + w - e);
      if (d < bd) {
        bd = d;
        bx = e - w;
      }
    }
    for (const e of [r[1], r[1] + r[3]]) {
      let d = Math.abs(ny - e);
      if (d < bdY) {
        bdY = d;
        by = e;
      }
      d = Math.abs(ny + h - e);
      if (d < bdY) {
        bdY = d;
        by = e - h;
      }
    }
  }
  return [bx != null ? bx : nx, by != null ? by : ny];
}

/**
 * 拉伸吸附：右下角拉伸时只有右/下边缘在动（左上角不动），
 * 把它们贴到候选边上。返回贴齐后的宽高；调用方自行做最小尺寸钳制。
 *
 * 拉伸时只有一条边在动，同样阈值的"粘住"距离感知比拖动弱得多
 * （拖动是整个窗口停住，拉伸只是边缘停一下），所以这里把阈值放大一倍、
 * 下限 32 物理像素，保证快速拉动也能明显感到粘住。
 */
export function resizeSnap(
  px: number,
  py: number,
  nw: number,
  nh: number,
  targets: SnapRect[],
  threshold: number
): [number, number] {
  if (threshold <= 0 || targets.length === 0) return [nw, nh];
  const th = Math.max(threshold * 2, 32);
  const right = px + nw;
  let bestX: number | null = null;
  let bestDX = th;
  for (const r of targets) {
    for (const edge of [r[0], r[0] + r[2]]) {
      const d = Math.abs(right - edge);
      if (d < bestDX) {
        bestDX = d;
        bestX = edge;
      }
    }
  }
  if (bestX != null) nw = bestX - px;
  const bottom = py + nh;
  let bestY: number | null = null;
  let bestDY = th;
  for (const r of targets) {
    for (const edge of [r[1], r[1] + r[3]]) {
      const d = Math.abs(bottom - edge);
      if (d < bestDY) {
        bestDY = d;
        bestY = edge;
      }
    }
  }
  if (bestY != null) nh = bestY - py;
  return [nw, nh];
}
