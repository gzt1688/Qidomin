/**
 * 把全局设置里的「分区标题颜色」应用到整个文档根节点。
 * 各窗口（分区悬浮窗 / 主界面面板卡片）加载设置后调用一次即可。
 * CSS 里用 var(--box-title-color, var(--green-title)) 回退到默认绿。
 */
export function applyBoxTitleColor(color?: string): void {
  const value = color && color.trim() ? color.trim() : "#1a9c5d";
  document.documentElement.style.setProperty("--box-title-color", value);
}
