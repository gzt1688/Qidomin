//! 系统图标 / 缩略图提取：`IShellItemImageFactory` → HBITMAP → PNG（内存缓存）。
//!
//! **为什么不把 base64 塞进 IPC 返回给前端**：一个 200 项的文件夹就是 200 张 PNG，
//! base64 之后是好几 MB 的字符串，每次刷新都要搬一遍、`JSON.parse` 一遍、每张再单独
//! 解码一遍 —— 界面必卡。现在后端只返回一个短 URL（走 `qidominicon://` 自定义协议），
//! 图片交给 WebView 自己拉；前端再配合 `loading="lazy"`，**只有看得见的那几张**
//! 才会真的触发提取。提取结果留在内存缓存里，来回滚动、反复刷新都是秒出。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};

use base64::Engine;
use windows::core::{HSTRING, Interface, PCWSTR};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DeleteDC, DeleteObject,
    DIB_RGB_COLORS, GetDIBits, GetObjectW, HBITMAP, SelectObject, HGDIOBJ,
};
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::UI::Shell::{
    IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
};

/// 自定义协议名。WebView 实际访问的是
/// `http://qidominicon.localhost/<管线版本>/<缓存代数>/<尺寸>/<base64url 编码的路径>`。
pub const PROTOCOL: &str = "qidominicon";

/// 图标生成管线的版本号。
///
/// **改了图标的生成方式（尺寸、裁剪规则等）就必须把它改一下**：
/// 地址变了，WebView 才不会拿 HTTP 缓存里的旧图 —— 否则改完代码、用户重启后
/// 看到的还是缓存里的老图标，等于白改。
const URL_VERSION: &str = "v6";

/// 提取缓存：路径 + 尺寸 → (插入序号, 文件修改时间, PNG 字节)
static ICON_CACHE: LazyLock<Mutex<HashMap<(String, u32), (u64, u64, Vec<u8>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 缓存容量上限
const ICON_CACHE_MAX: usize = 600;
/// 单调递增序号（LRU 淘汰用）
static ICON_SEQ: AtomicU64 = AtomicU64::new(0);

/// 文件修改时间（秒）。取不到就返回 0，退化成"一直用缓存"，不会比现在更差。
fn mtime_key(path: &str) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 缓存代数。清缓存时 +1，URL 里带上它 —— 这样地址本身变了，
/// WebView 就不会拿 HTTP 缓存里的旧图（否则"刚去掉小箭头，分区里还显示旧箭头"）。
/// 也正因为地址会变，响应可以放心地设长缓存。
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// 清空图标/缩略图缓存：外部环境变化（例如刚去掉快捷方式小箭头）后，
/// 让分区窗口重新向系统取一次图，避免还显示缓存里的旧箭头。
pub fn clear_cache() {
    if let Ok(mut c) = ICON_CACHE.lock() {
        c.clear();
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// 前端 `<img src>` 用的 URL。
///
/// 路径用 base64url（无填充）编码：字符集只有 `A-Za-z0-9-_`，不含 `/ + =`，
/// 中文、空格、反斜杠路径统统不用再做百分号编码，也不会被 URL 解析截断。
pub fn icon_url(path: &str, size: u32) -> String {
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(path.as_bytes());
    let gen = GENERATION.load(Ordering::Relaxed);
    format!("http://{PROTOCOL}.localhost/{URL_VERSION}/{gen}/{size}/{key}")
}

/// 把 [`icon_url`] 生成的地址还原成 (尺寸, 路径)。
/// 前两段（管线版本 / 缓存代数）只是用来绕过 HTTP 缓存，解析时直接跳过。
pub fn parse_icon_url(uri: &str) -> Option<(u32, String)> {
    let after_scheme = uri.split_once("://")?.1; // host/v4/0/96/xxxx
    let path = after_scheme.split_once('/')?.1; // v4/0/96/xxxx
    let rest = path.split_once('/')?.1; // 0/96/xxxx
    let rest = rest.split_once('/')?.1; // 96/xxxx
    // 前端重试时会在地址后面加 #n 触发重新加载；fragment 不该参与路径解码
    let rest = rest.split('#').next().unwrap_or(rest);
    let (size, key) = rest.split_once('/')?;
    let size: u32 = size.parse().ok()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key)
        .ok()?;
    Some((size, String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图标地址必须能原样解析回来：解析错一个字，整个分区的图标就全裂了。
    /// 中文、空格、反斜杠、`+ = ( )` 这些都得能过（base64url 编码就是为了这个）。
    #[test]
    fn icon_url_round_trip() {
        let cases = [
            r"C:\Users\Administrator\Desktop\软件\Bambu Studio.lnk",
            r"D:\中文 目录\a+b=c(1).png",
            r"C:\x\y.ico",
        ];
        for p in cases {
            let url = icon_url(p, 96);
            let (size, back) = parse_icon_url(&url).expect("URL 应能解析回来");
            assert_eq!(size, 96, "尺寸解析错误：{url}");
            assert_eq!(back, p, "路径解析错误：{url}");
        }
        // 顺手验证缓存代数确实进了地址（清缓存后要能换个地址，绕过 WebView 的 HTTP 缓存）
        let before = icon_url(cases[0], 96);
        clear_cache();
        assert_ne!(before, icon_url(cases[0], 96), "清缓存后地址应该变");
    }

    /// 回归：剪掉透明留白 + 等比缩放，内容不裁切、比例不变形。
    #[test]
    fn normalize_trims_padding_then_scales() {
        // 96×96 画布，中间只有 48×48 的真实图标（四周透明）：
        // 剪掉留白后等比放大到 96×96 —— 和"铺满画布的图标"视觉大小一致
        let mut padded = image::RgbaImage::new(96, 96);
        for y in 24..72 {
            for x in 24..72 {
                padded.put_pixel(x, y, image::Rgba([0, 200, 0, 255]));
            }
        }
        let out = contain_to_size(&padded, 96);
        assert_eq!(out.width(), 96);
        assert_eq!(out.height(), 96);

        // 铺满画布的图标：无留白可剪，原样等比
        let full = image::RgbaImage::from_pixel(96, 96, image::Rgba([0, 0, 255, 255]));
        let out2 = contain_to_size(&full, 96);
        assert_eq!(out2.width(), 96);
        assert_eq!(out2.height(), 96);

        // 宽图 200×50（照片类，无透明边）：只等比缩为 96×24，完整不裁
        let mut wide = image::RgbaImage::new(200, 50);
        for px in wide.pixels_mut() {
            *px = image::Rgba([255, 0, 0, 255]);
        }
        let out3 = contain_to_size(&wide, 96);
        assert_eq!(out3.width(), 96);
        assert_eq!(out3.height(), 24);
    }

    /// 回归：Shell 位图里那层几乎看不见的淡 alpha（a=20 铺满、内容只占中间），
    /// 阈值 128 必须能把它当留白剪掉，否则图标会显示成一半大。
    #[test]
    fn faint_layer_is_trimmed() {
        let mut img = image::RgbaImage::new(96, 96);
        for px in img.pixels_mut() {
            *px = image::Rgba([255, 0, 0, 20]);
        }
        for y in 24..72 {
            for x in 24..72 {
                img.put_pixel(x, y, image::Rgba([0, 200, 0, 255]));
            }
        }
        let out = contain_to_size(&img, 96);
        assert_eq!(out.width(), 96);
        assert_eq!(out.height(), 96);
        // 剪掉淡层后内容铺满整张画布：四角应是放大的图标内容（不透明），而不是 a=20 的淡红
        assert_eq!(out.get_pixel(0, 0).0[3], 255);
        assert_eq!(out.get_pixel(0, 0).0[1], 200);
    }
}

/// 取图标 PNG 字节（带缓存）。返回空数组表示提取失败，前端渲染成空白占位。
pub fn icon_png(path: &str, size: u32) -> Vec<u8> {
    let key = (path.to_lowercase(), size);
    let mt = mtime_key(path);
    if let Ok(cache) = ICON_CACHE.lock() {
        if let Some((_, cached_mt, bytes)) = cache.get(&key) {
            // 文件没变过就一直用缓存；被替换/图标改过（mtime 变了）才重新提取
            if *cached_mt == mt {
                return bytes.clone();
            }
        }
    }
    // 提取失败**不写缓存**：一次瞬时失败（文件正被占用、网络盘超时）如果缓存成空白，
    // 这个图标就"一辈子是白的"了，只有重启或清缓存才恢复。
    let Some(out) = extract_icon_png(path, size) else {
        return Vec::new();
    };
    if let Ok(mut cache) = ICON_CACHE.lock() {
        let seq = ICON_SEQ.fetch_add(1, Ordering::Relaxed);
        cache.insert(key, (seq, mt, out.clone()));
        // 超限只淘汰最久未用的 20%，而不是整表 clear ——
        // 全清会让当前屏幕上所有图标同时 miss，重新提取造成 CPU 尖峰。
        if cache.len() > ICON_CACHE_MAX {
            let mut items: Vec<((String, u32), u64)> =
                cache.iter().map(|(k, v)| (k.clone(), v.0)).collect();
            items.sort_unstable_by_key(|(_, s)| *s);
            for (k, _) in items.into_iter().take(ICON_CACHE_MAX / 5) {
                cache.remove(&k);
            }
        }
    }
    out
}

// ---------- 请求排队上限 ----------

/// 同时"已创建线程、正在等提取许可"的请求上限。
///
/// 协议回调原本是来一个请求就 spawn 一条线程、线程里再去抢并发许可 ——
/// 快速滚动一个几百项的文件夹会瞬间创建几百条线程（每条默认预留 1MB 栈），
/// 其中绝大多数阻塞在等待上。这里把排队长度封顶，超出的请求直接 503 让 WebView 稍后重试
/// （前端对 503 有自动重试，见 BoxWindow 的 onIconError）。
const MAX_PENDING: usize = 64;
static PENDING: AtomicUsize = AtomicUsize::new(0);

/// 为一次图标请求占位；返回 false 表示排队已满。
pub fn try_begin_request() -> bool {
    PENDING
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            if n >= MAX_PENDING {
                None
            } else {
                Some(n + 1)
            }
        })
        .is_ok()
}

/// 释放一个排队名额（与 [`try_begin_request`] 成对）
pub fn end_request() {
    let _ = PENDING.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
        Some(n.saturating_sub(1))
    });
}

// ---------- 提取并发闸门 ----------

/// 提取并发上限。缩略图要走 Shell 的缩略图处理器（解图片、渲染 3D 预览），
/// 一次放几十条线程进去只会互相抢 IO/CPU，反而更慢，还可能把资源管理器一起拖慢。
const MAX_CONCURRENT_EXTRACT: usize = 4;

struct Gate {
    running: Mutex<usize>,
    cv: Condvar,
}

static GATE: LazyLock<Gate> = LazyLock::new(|| Gate {
    running: Mutex::new(0),
    cv: Condvar::new(),
});

/// 提取许可；离开作用域自动归还
pub struct ExtractPermit;

impl Drop for ExtractPermit {
    fn drop(&mut self) {
        if let Ok(mut n) = GATE.running.lock() {
            *n = n.saturating_sub(1);
            GATE.cv.notify_one();
        }
    }
}

/// 申请一个提取许可（没空位就等）
pub fn acquire_extract_permit() -> ExtractPermit {
    let mut running = match GATE.running.lock() {
        Ok(g) => g,
        Err(_) => return ExtractPermit,
    };
    loop {
        if *running < MAX_CONCURRENT_EXTRACT {
            *running += 1;
            break;
        }
        // 锁中毒等异常情况：不再等，直接放行（宁可稍多一点并发，也别把请求卡死）
        running = match GATE.cv.wait(running) {
            Ok(g) => g,
            Err(_) => break,
        };
    }
    ExtractPermit
}

// ---------- 提取实现 ----------

fn extract_icon_png(path: &str, size: u32) -> Option<Vec<u8>> {
    let raw = extract_raw_bitmap(path, size)?;
    // 资源管理器同款：原样使用 Shell 的结果，超尺寸只做等比缩小。
    // 不剪白边、不居中裁切 —— 宽图 / 全景图按原比例完整呈现，与文件管理器一致。
    let img = contain_to_size(&raw, size.max(1));
    let mut out: Vec<u8> = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    img.write_to(&mut cursor, image::ImageFormat::Png).ok()?;
    Some(out)
}

/// 让 Shell 给一张图：有缩略图处理器的（图片、视频、3D 模型…）拿到真实预览，
/// 没有的（exe、快捷方式、文件夹…）自动退回普通图标。
/// 返回**原始位图**，没做任何裁剪/缩放。
fn extract_raw_bitmap(path: &str, size: u32) -> Option<image::RgbaImage> {
    // COM 初始化（线程内可能已初始化，忽略错误）
    unsafe {
        let _ = CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }

    let hstring = HSTRING::from(path);
    let item: IShellItem =
        unsafe { SHCreateItemFromParsingName(PCWSTR(hstring.as_ptr()), None).ok()? };
    let factory: IShellItemImageFactory = item.cast().ok()?;

    let sz = SIZE {
        cx: size as i32,
        cy: size as i32,
    };
    // 与桌面/资源管理器一致：不加 ICONONLY（那会强制只给图标，图片/3D 会变成通用图标），
    // 不加 CROPTOSQUARE（方形化我们自己来做）。
    let hbmp: HBITMAP = unsafe { factory.GetImage(sz, SIIGBF_BIGGERSIZEOK).ok()? };
    hbitmap_to_rgba(hbmp)
}

/// HBITMAP -> RGBA。GetImage 返回的位图句柄由调用方释放，这里统一 DeleteObject 防 GDI 泄漏。
fn hbitmap_to_rgba(hbmp: HBITMAP) -> Option<image::RgbaImage> {
    let out = hbitmap_to_rgba_raw(hbmp);
    unsafe {
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
    }
    out
}

fn hbitmap_to_rgba_raw(hbmp: HBITMAP) -> Option<image::RgbaImage> {
    unsafe {
        let mut bmp = BITMAP::default();
        let got = GetObjectW(
            HGDIOBJ(hbmp.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut core::ffi::c_void),
        );
        if got == 0 {
            return None;
        }
        let w = bmp.bmWidth;
        let h = bmp.bmHeight;
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // 自上而下
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;

        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let old = SelectObject(dc, HGDIOBJ(hbmp.0));
        let n = GetDIBits(
            dc,
            hbmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = SelectObject(dc, old);
        let _ = DeleteDC(dc);
        if n == 0 {
            return None;
        }

        // BGRA -> RGBA
        let mut rgba = Vec::with_capacity(buf.len());
        for px in buf.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        image::RgbaImage::from_raw(w as u32, h as u32, rgba)
    }
}

/// 剪裁用的"可见"阈值：alpha < 128 一律当作留白。
///
/// Shell 返回的位图里经常有一层**几乎看不见的淡 alpha** 铺满画布
/// （实测 .smprj 是 a>=8 时占满 100%、a>=64 时只占 50%×50%），
/// 阈值取低了就剪不掉，图标照样显示成一小块。取 128（半透明以上才算内容）
/// 既能剪掉这种淡层，又不会误剪正常的抗锯齿边缘。
const TRIM_ALPHA: u8 = 128;

/// 剪掉四周**看不见的透明留白**；没有可剪的就返回 None。
///
/// 注意这只裁掉空白边缘，不改动内容本身——图标内容该是什么样还是什么样。
fn trim_transparent(img: &image::RgbaImage) -> Option<image::RgbaImage> {
    let (w, h) = (img.width(), img.height());
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y).0[3] >= TRIM_ALPHA {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    // 整张图都够不着阈值（比如整体半透明的图标）→ 不剪，保持原样最安全
    if !any {
        return None;
    }
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    if cw == w && ch == h {
        return None; // 本来就没有留白
    }
    Some(image::imageops::crop_imm(img, x0, y0, cw, ch).to_image())
}

/// 图标归一化：**先剪掉看不见的透明留白，再等比缩放长边 = size**。
///
/// 两步都只是"去掉空白 + 整体缩放"，不重绘、不裁切内容：
/// 1. Shell 给的位图里，图标内容的实际占比差别极大（Bamboo Studio 的图标只占
///    画布中间一小块，开模费的 K 铺满整张）——不剪留白就会"大小不一"；
/// 2. 剪完统一等比缩放（小的放大、大的缩小），前端 `object-fit: contain` 居中，
///    宽图 / 全景图同样按原比例完整呈现（无内容损失，与资源管理器的自然观感一致）。
fn contain_to_size(src: &image::RgbaImage, size: u32) -> image::RgbaImage {
    let content = trim_transparent(src).unwrap_or_else(|| src.clone());
    let (w, h) = (content.width().max(1), content.height().max(1));
    let size = size.max(1);
    let m = w.max(h);
    if m == size {
        return content;
    }
    let nw = ((w as f32 * size as f32 / m as f32).round() as u32).clamp(1, size);
    let nh = ((h as f32 * size as f32 / m as f32).round() as u32).clamp(1, size);
    if nw == w && nh == h {
        return content;
    }
    // 缩小用盒式滤波（快且干净），放大用双线性（盒式放大块状感明显）
    if nw < w || nh < h {
        image::imageops::thumbnail(&content, nw, nh)
    } else {
        image::imageops::resize(&content, nw, nh, image::imageops::FilterType::Triangle)
    }
}
