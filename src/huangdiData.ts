/**
 * 黄帝内经 内置数据。
 *
 * 数据文件：`public/data/huangdi_neijing.json`（运行时 fetch，不打进 bundle）。
 * 内容：素问 81 篇（含遗篇72/73）+ 灵枢 81 篇 + 外经（清·陈士铎《外经微言》）81 篇
 *       + 难经（《黄帝八十一难经》）81 篇，共 324 篇、约 25.7 万字。
 *
 * 原文整理自 HarmonyQi 项目内置数据；素问遗篇 72《刺法论》/73《本病论》
 * 据通行本补齐。
 */

export type HuangdiBook = "素问" | "灵枢" | "外经" | "难经";

export const BOOK_ORDER: HuangdiBook[] = ["素问", "灵枢", "外经", "难经"];

export interface HuangdiChapter {
  book: HuangdiBook;
  /** 篇序号（书内） */
  no: number;
  /** 篇名，如 "上古天真论篇第一" */
  title: string;
  /** 正文（段落以换行分隔） */
  text: string;
}

export interface HuangdiMeta {
  name: string;
  suwen_chapters: number;
  lingshu_chapters: number;
  waijing_chapters: number;
  nanjing_chapters: number;
  total_chapters: number;
  total_chars: number;
  note: string;
}

export interface HuangdiData {
  meta: HuangdiMeta;
  chapters: HuangdiChapter[];
}

let cache: Promise<HuangdiData> | null = null;

/** 加载内经数据（只加载一次，之后复用缓存） */
export function loadHuangdi(): Promise<HuangdiData> {
  if (!cache) {
    cache = fetch("data/huangdi_neijing.json", { cache: "no-cache" })
      .then((r) => {
        if (!r.ok) throw new Error(`加载黄帝内经数据失败：HTTP ${r.status}`);
        return r.json();
      })
      .then((d) => d as HuangdiData);
  }
  return cache;
}

/** 按书统计篇数，用于标题栏展示 */
export async function bookCounts(): Promise<Record<string, number>> {
  const d = await loadHuangdi();
  const m = d.meta;
  return {
    素问: m.suwen_chapters,
    灵枢: m.lingshu_chapters,
    外经: m.waijing_chapters,
    难经: m.nanjing_chapters,
  };
}

/** 章节唯一标识 */
export function chapterKey(c: HuangdiChapter): string {
  return `${c.book}-${c.no}`;
}

/** 随机取一篇（可排除指定篇，避免连续重复） */
export async function randomChapter(
  excludeKey?: string
): Promise<HuangdiChapter | null> {
  const d = await loadHuangdi();
  if (!d.chapters.length) return null;
  let pick: HuangdiChapter;
  do {
    pick = d.chapters[Math.floor(Math.random() * d.chapters.length)];
  } while (d.chapters.length > 1 && chapterKey(pick) === excludeKey);
  return pick;
}

/** 按 书 / 篇名 / 正文 搜索；book 为空表示全部 */
export async function searchHuangdi(
  keyword: string,
  book?: HuangdiBook | ""
): Promise<HuangdiChapter[]> {
  const kw = (keyword || "").trim().toLowerCase();
  const d = await loadHuangdi();
  if (!kw) return [];
  return d.chapters.filter((c) => {
    if (book && c.book !== book) return false;
    return (
      c.title.toLowerCase().includes(kw) ||
      c.text.toLowerCase().includes(kw)
    );
  });
}

/** 当前书里按序号取相邻篇（上一篇/下一篇），供正文浏览翻页 */
export async function neighbor(
  cur: HuangdiChapter,
  dir: -1 | 1
): Promise<HuangdiChapter | null> {
  const d = await loadHuangdi();
  const same = d.chapters.filter((c) => c.book === cur.book);
  const idx = same.findIndex((c) => c.no === cur.no && c.book === cur.book);
  const next = same[idx + dir];
  return next ?? null;
}

/** 取某书第一篇 */
export async function firstOf(book: HuangdiBook): Promise<HuangdiChapter | null> {
  const d = await loadHuangdi();
  const same = d.chapters.filter((c) => c.book === book);
  return same[0] ?? null;
}
