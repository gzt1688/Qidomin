/**
 * 组件（便签 / 股票 / 黄历 / 诗词）右键菜单：弹在**独立置顶窗口**里。
 *
 * 以前菜单画在组件内部（`position: absolute` 的小面板），组件一矮菜单就被
 * 窗口边界裁掉，下面的项根本点不到。现在和分区菜单同一套做法：
 * 菜单是自己的窗口，尺寸按内容自适应，永远看得全。
 *
 * 分工：
 * - 组件窗口 = 数据与逻辑的 owner：把"菜单长什么样"打包成 `MenuItem[]` 发布出来，
 *   再接收"用户点了哪一项"；
 * - 菜单窗口 = 纯渲染器（`WidgetMenuWindow.tsx`），不含任何业务逻辑。
 *
 * 两边不直接 emit 对问（试过：菜单窗刚建好时问，经常问回一个空菜单，
 * 结果就是一条白条），而是**后端当中转站 + 仓库**：
 *
 * - 组件：挂载 / 状态一变就 `widget_menu_publish` 把菜单内容交给后端存一份；
 * - 菜单窗：首屏 `widget_menu_get` 直取（不依赖事件时序），之后靠广播更新；
 * - 索要刷新 / 点选：都走 `widget_menu_request` / `widget_menu_pick`，
 *   由后端 `emit_to` **定向**发给组件窗口，不会串到别的窗口。
 */
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

/** 组件的标识（同时也是菜单窗口的 key） */
export type WidgetKind = "notes" | "stock" | "huangli" | "poetry" | "huangdi" | "shanghan";

/** 菜单项：一个"能画出来"的描述，具体做什么由 owner 决定 */
export type MenuItem =
  /** 一行按钮（点完默认收起菜单） */
  | { t: "action"; id: string; label: string; danger?: boolean; primary?: boolean }
  /** 分组标题（灰字 + 上方细分割线） */
  | { t: "label"; text: string }
  /** 一排小胶囊：单选，选中项高亮 */
  | { t: "chips"; id: string; value: string; options: { value: string; label: string }[] }
  /** 背景色 + 不透明度（原生取色器 + 滑杆） */
  | { t: "color"; id: string; color: string; opacity: number };

/** 菜单窗口 → 组件：报上自己的内容高度，让后端定尺寸 */
export const EVT_REQUEST = "widget-menu-request";
/** 组件 → 菜单窗口：菜单内容 */
export const EVT_MODEL = "widget-menu-model";
/** 菜单窗口 → 组件：用户点了哪一项 */
export const EVT_PICK = "widget-menu-pick";

/** 打开（或收起）组件菜单 */
export function openWidgetMenu(kind: WidgetKind): Promise<void> {
  return invoke("open_widget_menu", { kind });
}

/** 菜单内容量完高度后调用：贴到组件外侧 + 定尺寸 + 显示 */
export function fitWidgetMenu(kind: WidgetKind, height: number): Promise<void> {
  return invoke("fit_widget_menu", { kind, height });
}

/** 收起菜单 */
export function closeWidgetMenu(kind: WidgetKind): Promise<void> {
  return invoke("close_widget_menu", { kind });
}

/** 菜单窗口首屏直取菜单内容（后端仓库里那一份） */
export async function getMenuModel(kind: WidgetKind): Promise<MenuItem[]> {
  const v = await invoke<MenuItem[]>("widget_menu_get", { kind });
  return Array.isArray(v) ? v : [];
}

/** 组件发布菜单内容（每次状态变了都要重发一次） */
export function publishMenu(kind: WidgetKind, model: MenuItem[]): void {
  void invoke("widget_menu_publish", { kind, model }).catch(() => {});
}

/** 菜单窗口索要内容（打开时 / 重新获得焦点时都会发一次） */
export function onMenuRequest(
  kind: WidgetKind,
  cb: () => void
): Promise<UnlistenFn> {
  return listen<{ kind: WidgetKind }>(EVT_REQUEST, (e) => {
    if (e.payload?.kind === kind) cb();
  });
}

/** 菜单窗口回报"用户点了哪一项" */
export function onMenuPick(
  kind: WidgetKind,
  cb: (id: string, value?: string | number) => void
): Promise<UnlistenFn> {
  return listen<{ kind: WidgetKind; id: string; value?: string | number }>(
    EVT_PICK,
    (e) => {
      if (e.payload?.kind !== kind) return;
      cb(e.payload.id, e.payload.value);
    }
  );
}

/** 菜单窗口 → 组件：报一个选项值（胶囊 / 颜色 / 滑杆） */
export function emitPick(
  kind: WidgetKind,
  id: string,
  value?: string | number
): void {
  void invoke("widget_menu_pick", { kind, id, value }).catch(() => {});
}

/** 菜单窗口：向组件索要最新内容（后端定向转发，不会吵到别的窗口） */
export function requestModel(kind: WidgetKind): void {
  void invoke("widget_menu_request", { kind }).catch(() => {});
}
