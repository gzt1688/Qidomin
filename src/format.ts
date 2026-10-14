/**
 * 展示用的文件名：与桌面保持一致。
 * 快捷方式（.lnk / .url）只显示主名，不带后缀 —— 后缀是技术细节，桌面上也不显示。
 * 其他类型默认保留后缀，方便区分同名文件；用户可在设置里开启「不显示后缀名」。
 */

/**
 * 「分区文件不显示后缀名」开关（模块级，随设置加载刷新）。
 * 各窗口加载设置后调用 setHideFileExtensions 同步；设置变更由后端
 * 广播 settings-changed 事件驱动各窗口重载。
 */
let hideExt = false;

export function setHideFileExtensions(v: boolean): void {
  hideExt = !!v;
}

export function displayFileName(name: string, isDir = false): string {
  if (!hideExt) return name.replace(/\.(lnk|url)$/i, "");
  // 开启后隐藏文件的最后一段后缀；文件夹不动（文件夹的「点」多半是名字本身）。
  // 后缀段限长 10：避免把「2026.09.15」这类纯数字名字误当后缀截掉主体。
  if (!isDir) {
    const dot = name.lastIndexOf(".");
    if (dot > 0 && name.length - dot - 1 <= 10) return name.slice(0, dot);
  }
  return name.replace(/\.(lnk|url)$/i, "");
}
