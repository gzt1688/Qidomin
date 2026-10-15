import { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import SettingsView from "./components/SettingsView";
import OrganizePreview from "./components/OrganizePreview";

export default function App() {
  const [statusText, setStatusText] = useState("");
  const [statusKind, setStatusKind] = useState<"ok" | "err" | "">("");
  const [refreshKey, setRefreshKey] = useState(0);

  const notify = useCallback((text: string, kind: "ok" | "err" | "") => {
    setStatusText(text);
    setStatusKind(kind);
  }, []);

  const doMinimize = useCallback(() => {
    getCurrentWindow().minimize();
  }, []);

  const doHide = useCallback(() => {
    getCurrentWindow().hide();
  }, []);

  useEffect(() => {
    notify("就绪", "ok");
  }, [notify]);

  return (
    <div className="app-shell">
      <div className="settings-topbar">
        <span className="app-title">Qidomin</span>
        <span className="settings-subtitle">桌面分区整理</span>
        <div style={{ flex: 1 }} />
        <button className="icon-btn" title="最小化" onClick={doMinimize}>
          ─
        </button>
        <button className="icon-btn close" title="隐藏到托盘" onClick={doHide}>
          ✕
        </button>
      </div>

      <SettingsView notify={notify} refreshKey={refreshKey} onChange={setRefreshKey} />

      {/* 托盘"一键整理" / 鼠标热键触发的整理预览（plan → 确认 → commit → undo） */}
      <OrganizePreview notify={notify} />

      <div className={`status-bar ${statusKind}`}>{statusText}</div>
    </div>
  );
}
