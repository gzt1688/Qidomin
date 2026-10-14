import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getBoxFiles, getBoxes, getSettings, openFile, organizeBox } from "../api";
import { displayFileName, setHideFileExtensions } from "../format";
import { applyBoxTitleColor } from "../theme";
import type { BoxInfo, DesktopFile } from "../types";

interface PanelViewProps {
  refreshKey: number;
  notify: (text: string, kind: "ok" | "err" | "") => void;
}

function formatSize(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} MB`;
  return `${(size / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

export default function PanelView({ refreshKey, notify }: PanelViewProps) {
  const [boxes, setBoxes] = useState<BoxInfo[]>([]);
  const [boxFiles, setBoxFiles] = useState<Record<number, DesktopFile[]>>({});

  const loadBoxes = useCallback(async () => {
    try {
      const list = await getBoxes();
      getSettings()
        .then((s) => {
          applyBoxTitleColor(s.boxTitleColor);
          // 「分区文件隐藏后缀名」开关（displayFileName 读这个模块级标记）
          setHideFileExtensions(s.hideFileExtensions === true);
        })
        .catch(() => {});
      setBoxes(list);
      const filesMap: Record<number, DesktopFile[]> = {};
      for (const box of list) {
        filesMap[box.id] = await getBoxFiles(box.name);
      }
      setBoxFiles(filesMap);
    } catch (e) {
      notify(`加载分区失败：${String(e)}`, "err");
    }
  }, [notify]);

  useEffect(() => {
    loadBoxes();
  }, [loadBoxes, refreshKey]);

  // 设置页切换「分区文件不显示后缀名」→ 重载设置与文件列表，主窗口面板实时生效
  useEffect(() => {
    const un = listen<boolean>("settings-changed", () => {
      loadBoxes();
    });
    return () => {
      un.then((f) => f());
    };
  }, [loadBoxes]);

  const doOrganizeBox = useCallback(
    async (box: BoxInfo) => {
      try {
        const result = await organizeBox(box.id);
        notify(
          result.moved > 0
            ? `已将 ${result.moved} 个不匹配文件移出「${box.name}」`
            : `「${box.name}」内容全部匹配，无需调整`,
          "ok"
        );
        loadBoxes();
      } catch (e) {
        notify(`整理分区失败：${String(e)}`, "err");
      }
    },
    [notify, loadBoxes]
  );

  const doOpenFile = useCallback(async (path: string) => {
    try {
      await openFile(path);
    } catch (e) {
      /* 打开失败不弹窗，但要留下日志便于排查 */
      console.error("打开文件失败", path, e);
    }
  }, []);

  return (
    <div className="box-grid">
      {boxes.map((box) => {
        const files = boxFiles[box.id] || [];
        return (
          <div className="box-card" key={box.id}>
            <div className="box-head">
              <span className="box-color-dot" style={{ background: box.color }} />
              <span className="box-name">{box.name}</span>
              <span className="box-count">{box.fileCount} 项</span>
            </div>
            <ul className="box-file-list">
              {files.slice(0, 12).map((f) => (
                <li
                  key={f.path}
                  title={`${f.path}（${formatSize(f.size)}）`}
                  onDoubleClick={() => doOpenFile(f.path)}
                >
                  {f.isDir ? "📁 " : "📄 "}
                  {displayFileName(f.name, f.isDir)}
                </li>
              ))}
              {files.length === 0 && (
                <li style={{ color: "#9aabb2" }}>空空如也</li>
              )}
            </ul>
            <div className="box-foot">
              <button className="btn small ghost" onClick={() => doOrganizeBox(box)}>
                整理此分区
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}