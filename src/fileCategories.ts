// 搜索「文件类型大类」的默认配置（后缀名均小写、不含点）。
// 设置页会按此表列出每个大类下的具体后缀供用户勾选，默认全部勾选。
// 搜索面板第二行按这些大类出过滤按钮。

export interface FileCategory {
  /** 固定 key，存设置时用它做键 */
  key: string;
  /** 展示名 */
  label: string;
  /** 该大类默认包含的后缀名 */
  exts: string[];
}

export const FILE_CATEGORIES: FileCategory[] = [
  {
    key: "image",
    label: "图片",
    exts: [
      "jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "tif", "tiff",
      "heic", "ico", "psd", "raw", "cr2", "cr3", "arw", "avif",
    ],
  },
  {
    key: "video",
    label: "视频",
    exts: [
      "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg",
      "mpeg", "rm", "rmvb", "ts", "3gp", "vob",
    ],
  },
  {
    key: "audio",
    label: "音频",
    exts: [
      "mp3", "wav", "flac", "aac", "ogg", "wma", "m4a", "ape", "opus",
      "wv", "ac3", "mid", "midi",
    ],
  },
  {
    key: "doc",
    label: "文档",
    exts: [
      "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pdf", "txt", "md",
      "rtf", "csv", "wps", "odt", "ods", "odp", "epub", "mobi", "pages",
      "key", "numbers", "tex",
    ],
  },
  {
    key: "archive",
    label: "压缩包",
    exts: [
      "zip", "rar", "7z", "tar", "gz", "bz2", "z", "iso", "cab", "xz",
      "tgz", "war", "jar", "lz", "lz4",
    ],
  },
  {
    key: "model3d",
    label: "3D模型",
    exts: [
      "obj", "fbx", "stl", "blend", "max", "ma", "mb", "3ds", "dae",
      "glb", "gltf", "skp", "step", "stp", "iges", "igs", "dwg", "dxf",
      "c4d", "3mf", "3dm", "3dmbak",
      // 精雕（JDPaint / SurfMill，smprj 为新版工程文件）
      "smprj", "escam", "jd", "jdp", "jcd",
      // 拓竹（Bambu Studio）
      "bambu", "gcode", "bgcode",
    ],
  },
  {
    key: "program",
    label: "程序",
    exts: [
      "exe", "msi", "bat", "cmd", "ps1", "lnk", "appref-ms", "appx",
      "msix", "com", "jar",
    ],
  },
];

// 类目采用内置默认表：每个大类默认全部启用，不做逐个后缀的勾选存储，
// 避免多窗口间设置同步带来的"改了不生效"问题。
