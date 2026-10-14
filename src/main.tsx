import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";

// 按窗口类型动态加载：分区窗口只需要 BoxWindow，设置窗口只需要 App。
// 拆成独立 chunk 后，每个窗口首次加载/解析的 JS 更少，窗口出现更快。
const params = new URLSearchParams(window.location.search);
const boxId = params.get("box");
const boxMenuId = params.get("boxmenu");
const isNotes = params.get("notes");
const isStocks = params.get("stocks");
const isHuangli = params.get("huangli");
const isPoetry = params.get("poetry");
const isHuangdi = params.get("huangdi");
const isShanghan = params.get("shanghan");
const isSearch = params.has("search");
const widgetMenu = params.get("widgetmenu");

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

if (boxMenuId) {
  const BoxMenuWindow = lazy(() => import("./components/BoxMenuWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <BoxMenuWindow boxId={Number(boxMenuId)} />
      </Suspense>
    </React.StrictMode>
  );
} else if (widgetMenu) {
  // 便签 / 股票 / 黄历 / 诗词的右键菜单：独立置顶窗口，不再被组件高度裁掉
  const WidgetMenuWindow = lazy(() => import("./components/WidgetMenuWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <WidgetMenuWindow
          kind={widgetMenu as "notes" | "stock" | "huangli" | "poetry" | "huangdi" | "shanghan"}
        />
      </Suspense>
    </React.StrictMode>
  );
} else if (boxId) {
  const BoxWindow = lazy(() => import("./components/BoxWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <BoxWindow boxId={Number(boxId)} />
      </Suspense>
    </React.StrictMode>
  );
} else if (isNotes) {
  const NotesWindow = lazy(() => import("./components/NotesWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <NotesWindow />
      </Suspense>
    </React.StrictMode>
  );
} else if (isStocks) {
  const StockWindow = lazy(() => import("./components/StockWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <StockWindow />
      </Suspense>
    </React.StrictMode>
  );
} else if (isHuangli) {
  const HuangliWindow = lazy(() => import("./components/HuangliWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <HuangliWindow />
      </Suspense>
    </React.StrictMode>
  );
} else if (isPoetry) {
  const PoetryWindow = lazy(() => import("./components/PoetryWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <PoetryWindow />
      </Suspense>
    </React.StrictMode>
  );
} else if (isHuangdi) {
  const HuangdiWindow = lazy(() => import("./components/HuangdiWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <HuangdiWindow />
      </Suspense>
    </React.StrictMode>
  );
} else if (isShanghan) {
  const ShanghanWindow = lazy(() => import("./components/ShanghanWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <ShanghanWindow />
      </Suspense>
    </React.StrictMode>
  );
} else if (isSearch) {
  const SearchWindow = lazy(() => import("./components/SearchWindow"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <SearchWindow />
      </Suspense>
    </React.StrictMode>
  );
} else {
  const App = lazy(() => import("./App"));
  root.render(
    <React.StrictMode>
      <Suspense fallback={null}>
        <App />
      </Suspense>
    </React.StrictMode>
  );
}
