/**
 * 饼型菜单内置图标：精心设计的扁平圆底图标（渐变底 + 白色 glyph）。
 * 渲染成 data URL 直接当 radialIcon 用，无需读文件、离线可用。
 * 设置页和饼型右键菜单共用这一份。
 */
export const RADIAL_PRESET_ICONS: { key: string; label: string; svg: string }[] = [
  {
    key: "folder",
    label: "文件夹",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fbbf24"/><stop offset="1" stop-color="#f59e0b"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M10 17a3 3 0 0 1 3-3h7l3.5 3.5H35a3 3 0 0 1 3 3V31a3 3 0 0 1-3 3H13a3 3 0 0 1-3-3V17z" fill="#fff" opacity=".95"/><path d="M10 22h28v9a3 3 0 0 1-3 3H13a3 3 0 0 1-3-3v-9z" fill="#fff"/></svg>',
  },
  {
    key: "apps",
    label: "应用",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#60a5fa"/><stop offset="1" stop-color="#3b82f6"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><rect x="11" y="11" width="11.5" height="11.5" rx="3.5" fill="#fff"/><rect x="25.5" y="11" width="11.5" height="11.5" rx="3.5" fill="#fff" opacity=".82"/><rect x="11" y="25.5" width="11.5" height="11.5" rx="3.5" fill="#fff" opacity=".82"/><rect x="25.5" y="25.5" width="11.5" height="11.5" rx="3.5" fill="#fff"/></svg>',
  },
  {
    key: "ai",
    label: "AI 智能助手",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#a855f7"/><stop offset=".55" stop-color="#7c3aed"/><stop offset="1" stop-color="#ec4899"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M23 9c1.1 6.2 4.3 9.4 10.5 10.5C27.3 20.6 24.1 23.8 23 30c-1.1-6.2-4.3-9.4-10.5-10.5C18.7 18.4 21.9 15.2 23 9z" fill="#fff"/><path d="M33.5 27c.6 3.4 2.4 5.2 5.8 5.8-3.4.6-5.2 2.4-5.8 5.8-.6-3.4-2.4-5.2-5.8-5.8 3.4-.6 5.2-2.4 5.8-5.8z" fill="#fff" opacity=".92"/><circle cx="14" cy="35" r="2" fill="#fff" opacity=".75"/></svg>',
  },
  {
    key: "cube3d",
    label: "3D 建模",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#38bdf8"/><stop offset="1" stop-color="#2563eb"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M24 9l13 7v16l-13 7-13-7V16l13-7z" fill="#fff" opacity=".95"/><path d="M24 9l13 7-13 7-13-7 13-7z" fill="#dbeafe"/><path d="M24 23v16l13-7V16l-13 7z" fill="#93c5fd"/><path d="M11 16l13 7v16l-13-7V16z" fill="#fff" opacity=".65"/></svg>',
  },
  {
    key: "game",
    label: "游戏",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#a78bfa"/><stop offset="1" stop-color="#8b5cf6"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M17 18h14a8 8 0 0 1 8 8c0 3.3-2.2 5-4.5 5-1.8 0-3-1-4.5-2.5-.9-.9-2-1.5-6-1.5s-5.1.6-6 1.5C16.5 30 15.3 31 13.5 31 11.2 31 9 29.3 9 26a8 8 0 0 1 8-8z" fill="#fff"/><path d="M16 23v6M13 26h6" stroke="#8b5cf6" stroke-width="2.4" stroke-linecap="round"/><circle cx="31" cy="24.4" r="1.9" fill="#8b5cf6"/><circle cx="35" cy="27.6" r="1.9" fill="#8b5cf6"/></svg>',
  },
  {
    key: "music",
    label: "音乐",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f472b6"/><stop offset="1" stop-color="#ec4899"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M20 32V15l14-3v16" stroke="#fff" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" fill="none"/><circle cx="16.5" cy="32" r="4" fill="#fff"/><circle cx="30.5" cy="28" r="4" fill="#fff"/></svg>',
  },
  {
    key: "video",
    label: "视频",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#f87171"/><stop offset="1" stop-color="#ef4444"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><circle cx="24" cy="24" r="14" fill="#fff" opacity=".22"/><path d="M20 17.5 32 24l-12 6.5v-13z" fill="#fff"/></svg>',
  },
  {
    key: "photo",
    label: "图片",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#22d3ee"/><stop offset="1" stop-color="#06b6d4"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><rect x="10" y="12" width="28" height="24" rx="4" fill="#fff"/><circle cx="18" cy="20" r="3" fill="#06b6d4"/><path d="M12 33l9-9 6 6 4-4 7 7v1a2 2 0 0 1-2 2H14a2 2 0 0 1-2-2v-1z" fill="#22d3ee"/></svg>',
  },
  {
    key: "download",
    label: "下载",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#34d399"/><stop offset="1" stop-color="#10b981"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M24 12v14m0 0 6-6m-6 6-6-6" stroke="#fff" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/><path d="M13 30v3a4 4 0 0 0 4 4h14a4 4 0 0 0 4-4v-3" stroke="#fff" stroke-width="3.2" stroke-linecap="round" fill="none"/></svg>',
  },
  {
    key: "work",
    label: "工作",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#818cf8"/><stop offset="1" stop-color="#4f46e5"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><rect x="10" y="17" width="28" height="19" rx="4" fill="#fff"/><path d="M19 17v-3a4 4 0 0 1 4-4h2a4 4 0 0 1 4 4v3" stroke="#fff" stroke-width="3" fill="none"/><path d="M10 25h28" stroke="#4f46e5" stroke-width="2.4"/><rect x="21" y="23" width="6" height="5" rx="1.4" fill="#4f46e5"/></svg>',
  },
  {
    key: "note",
    label: "笔记",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fbbf24"/><stop offset="1" stop-color="#f59e0b"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><rect x="12" y="10" width="24" height="28" rx="4" fill="#fff"/><path d="M17 18h14M17 24h14M17 30h9" stroke="#f59e0b" stroke-width="2.6" stroke-linecap="round"/></svg>',
  },
  {
    key: "tools",
    label: "工具",
    svg: '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#94a3b8"/><stop offset="1" stop-color="#64748b"/></linearGradient></defs><rect x="2" y="2" width="44" height="44" rx="14" fill="url(#g)"/><path d="M30 12a8 8 0 0 0-9.8 10.8L12 31a4.2 4.2 0 0 0 6 6l8.2-8.2A8 8 0 0 0 37 19l-4.5 4.5-4-1-1-4L32 14a8 8 0 0 0-2-2z" fill="#fff"/></svg>',
  },
];

/** SVG 源码 → data URL（encodeURIComponent 比 btoa 稳，不怕中文/特殊字符） */
export function svgToDataUrl(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}
