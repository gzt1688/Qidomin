/**
 * 伤寒论 内置数据。
 *
 * 数据文件：`public/data/shanghanlun.json`（运行时 fetch，不打进 bundle）。
 * 内容：宋本《伤寒论》398 条（辨太阳病上中下 / 阳明 / 少阳 / 太阴 / 少阴 /
 *       厥阴 / 霍乱 / 阴阳易差后劳复 共 10 章），约 2.98 万字。
 *
 * 原文整理自 HarmonyQi 项目内置数据。
 */

export type ShanghanBook = "伤寒论";

export const BOOK_ORDER: ShanghanBook[] = ["伤寒论"];

export interface ShanghanChapter {
  book: ShanghanBook;
  /** 条文序号（1~398） */
  no: number;
  /** 条目名，如 "第1条 · 辨太阳病脉证并治上" */
  title: string;
  /** 条文正文 */
  text: string;
}

export interface ShanghanMeta {
  name: string;
  chapters: number;
  total_chars: number;
  note: string;
}

export interface ShanghanData {
  meta: ShanghanMeta;
  chapters: ShanghanChapter[];
}

let cache: Promise<ShanghanData> | null = null;

/** 加载伤寒论数据（只加载一次，之后复用缓存） */
export function loadShanghan(): Promise<ShanghanData> {
  if (!cache) {
    cache = fetch("data/shanghanlun.json", { cache: "no-cache" })
      .then((r) => {
        if (!r.ok) throw new Error(`加载伤寒论数据失败：HTTP ${r.status}`);
        return r.json();
      })
      .then((d) => d as ShanghanData);
  }
  return cache;
}

/** 条目唯一标识 */
export function chapterKey(c: ShanghanChapter): string {
  return `${c.book}-${c.no}`;
}

/** 随机取一条（可排除指定条，避免连续重复） */
export async function randomChapter(
  excludeKey?: string
): Promise<ShanghanChapter | null> {
  const d = await loadShanghan();
  if (!d.chapters.length) return null;
  let pick: ShanghanChapter;
  do {
    pick = d.chapters[Math.floor(Math.random() * d.chapters.length)];
  } while (d.chapters.length > 1 && chapterKey(pick) === excludeKey);
  return pick;
}

/** 按 条名 / 正文 搜索 */
export async function searchShanghan(
  keyword: string
): Promise<ShanghanChapter[]> {
  const kw = (keyword || "").trim().toLowerCase();
  const d = await loadShanghan();
  if (!kw) return [];
  return d.chapters.filter((c) => {
    return (
      c.title.toLowerCase().includes(kw) ||
      c.text.toLowerCase().includes(kw)
    );
  });
}

/** 按全书顺序取相邻条（上一篇/下一篇），供正文浏览翻页 */
export async function neighbor(
  cur: ShanghanChapter,
  dir: -1 | 1
): Promise<ShanghanChapter | null> {
  const d = await loadShanghan();
  const idx = d.chapters.findIndex((c) => c.no === cur.no && c.book === cur.book);
  const next = d.chapters[idx + dir];
  return next ?? null;
}

/** 取第一篇 */
export async function firstOf(): Promise<ShanghanChapter | null> {
  const d = await loadShanghan();
  return d.chapters[0] ?? null;
}
