//! 桌面快捷方式小箭头（快捷方式叠加图标）开关。
//!
//! 原理：explorer 绘制快捷方式左下角的小箭头时，会读取
//!   `...\Explorer\Shell Icons` 的 29 号图标槽（键见下方常量），
//! 把 29 指向一个透明图标即可去掉箭头，删除该值即可恢复原样。
//!
//! **写在哪里 —— 最容易踩坑的一条**：以 **系统级 HKLM** 为准。
//! 实测（Win11 26H1 / Build 28000）explorer 会**忽略当前用户级 HKCU** 的覆盖：
//! 注册表里明明写进去了、桌面上箭头却纹丝不动。所以 HKLM 是必需项（需要管理员权限：
//! 已提权则无弹窗，否则请求一次 UAC），HKCU 只是顺手一起写，兼容旧版本。
//!
//! 两条路径：
//!
//! 1) **常规**：写注册表 → 作废并重建图标缓存 → 再通知一轮
//!    （**不重启 explorer**，所以不会闪黑屏；让 shell 就地重新合成图标）：
//!      a. `SHChangeNotify(SHCNE_ASSOCCHANGED)` 让 shell 作废并重建图标缓存；
//!      b. 对桌面 / 公共桌面 / 开始菜单 / 任务栏固定项里的**每一个快捷方式**单独发
//!         `SHCNE_UPDATEITEM`，强制 shell 只重新解析并重绘这些项（这是"不闪"的关键，
//!         桌面不会整体重画）；
//!      c. 对桌面等目录发 `SHCNE_UPDATEDIR`；
//!      d. `ie4uinit.exe -show / -ClearIconCache` 重建图标缓存数据库；
//!      e. 600ms 后再补发一轮，覆盖 explorer 正忙/刚启动的情况。
//!
//! 2) **强制生效** [`force_apply`]：explorer 一直复用 `iconcache_*.db` 里的旧箭头图时，
//!    只刷通知是没用的。这条路径会**在 explorer 退出后删掉图标缓存**、再重启 explorer。
//!    （注意：缓存文件在 explorer 运行期间被占用，删不掉 ——
//!    这也是"手动重启资源管理器却依然没用"的原因。）
//!
//! 不使用删除 `lnkfile\IsShortcut` 的老办法 —— 那种做法会让 Win+X、开始菜单、
//! 任务栏固定项出现异常。

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::Duration;

use tauri::AppHandle;

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{
    SHCNE_ASSOCCHANGED, SHCNE_ID, SHCNE_UPDATEDIR, SHCNE_UPDATEITEM, SHCNF_FLUSH,
    SHCNF_IDLIST, SHCNF_PATHW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, SHChangeNotify,
    ShellExecuteExW,
};
use windows::core::{HSTRING, PCWSTR};

/// 当前用户级的图标覆盖（优先，且无需管理员权限）
const HKCU_KEY: &str = r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Shell Icons";
/// 系统级同名键，早期版本曾写入这里，恢复时需要顺带清理
const HKLM_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\Shell Icons";
/// 29 号图标槽 = 快捷方式叠加箭头
const ARROW_VALUE: &str = "29";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn system_root() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string())
}

// ---------------------------------------------------------------------------
// 透明图标文件
// ---------------------------------------------------------------------------

/// 覆盖图标会按当前 DPI 请求不同像素尺寸（100%→16、125%→20、150%→24、200%→32…），
/// 尺寸对不上时 Windows 会把透明图缩放，重采样后容易再出黑块。
/// 这里把所有常见尺寸都生成一遍，任何 DPI 下都能取到尺寸完全匹配的那一张。
const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 128];

/// AND 掩码每行按 4 字节对齐，掩码总字节数
fn ico_mask_len(dim: u32) -> usize {
    let stride = ((dim + 31) / 32) * 4; // 每行比特数向上取整到 4 字节
    (stride * dim) as usize
}

/// 单张图像的字节数：BITMAPINFOHEADER(40) + BGRA 像素 + AND 掩码
fn ico_image_len(dim: u32) -> usize {
    40 + (dim * dim * 4) as usize + ico_mask_len(dim)
}

/// 整个 .ico 文件的字节数（6 字节 ICONDIR + 每张 16 字节目录项 + 各张图像）
fn blank_ico_total() -> usize {
    6 + 16 * ICO_SIZES.len() + ICO_SIZES.iter().map(|d| ico_image_len(*d)).sum::<usize>()
}

/// 自己生成全透明的 .ico，不依赖系统资源编号（`imageres.dll,197` 那类写法在部分
/// Win11 版本上会被忽略），各个 Windows 版本表现一致。
fn blank_ico_bytes() -> Vec<u8> {
    let mut b: Vec<u8> = Vec::with_capacity(blank_ico_total());
    // ICONDIR
    b.extend_from_slice(&0u16.to_le_bytes()); // reserved
    b.extend_from_slice(&1u16.to_le_bytes()); // type: 1 = icon
    b.extend_from_slice(&(ICO_SIZES.len() as u16).to_le_bytes()); // count
    // ICONDIRENTRY × n
    let mut offset = 6 + 16 * ICO_SIZES.len() as u32;
    for &d in ICO_SIZES.iter() {
        b.push(d as u8); // width（最大 48，不需要 256 的 0 写法）
        b.push(d as u8); // height
        b.push(0); // colorCount
        b.push(0); // reserved
        b.extend_from_slice(&1u16.to_le_bytes()); // planes
        b.extend_from_slice(&32u16.to_le_bytes()); // bitCount
        b.extend_from_slice(&(ico_image_len(d) as u32).to_le_bytes()); // bytesInRes
        b.extend_from_slice(&offset.to_le_bytes()); // imageOffset
        offset += ico_image_len(d) as u32;
    }
    // 图像数据 × n（32 位 DIB：高度写双倍，含 AND mask）
    for &d in ICO_SIZES.iter() {
        let px = (d * d * 4) as usize;
        let mask = ico_mask_len(d);
        b.extend_from_slice(&40u32.to_le_bytes()); // biSize
        b.extend_from_slice(&(d as i32).to_le_bytes()); // biWidth
        b.extend_from_slice(&((d * 2) as i32).to_le_bytes()); // biHeight
        b.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
        b.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
        b.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        b.extend_from_slice(&(px as u32).to_le_bytes()); // biSizeImage
        b.extend_from_slice(&[0u8; 16]); // xppm / yppm / clrUsed / clrImportant
        // 像素 BGRA：RGB 全 0，但 alpha 必须是 **1 而不是 0**。
        // 整幅 alpha 全 0 会被编码器 / 渲染路径当成"空白图"，
        // 退化成一张不透明黑位图 —— 于是箭头位置出现一个黑色方块。
        // 1/255 ≈ 0.4% 的不透明度肉眼完全看不出来，却足以让 Windows
        // 认定这是一张合法的、带 alpha 通道的图，不再走黑块退化分支。
        for _ in 0..(d * d) {
            b.extend_from_slice(&[0, 0, 0, 1]);
        }
        // AND 掩码必须全 1：掩码里 1 = 透明。全 0 的掩码在不看 alpha 的渲染路径
        // （覆盖图标就是走这条路径）会按"全不透明"画出来 —— 表现为图标左下角
        // 多出一块不透明方块，看着像图标被裁掉一块。
        b.resize(b.len() + mask, 0xFF);
    }
    b
}

fn blank_icon_path() -> std::path::PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| system_root());
    std::path::Path::new(&base).join("Qidomin").join("blank.ico")
}

/// 确保透明图标文件存在且内容正确（缺失、被改动或旧版本写坏的文件都会重写），
/// 返回其绝对路径
fn ensure_blank_ico() -> Result<String, String> {
    let p = blank_icon_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let want = blank_ico_bytes();
    // 逐字节比对：图标内容本身就是判断"要不要重写"的依据，
    // 这样以后调整图标（尺寸/掩码）时，用户机器上的旧文件一定会被替换掉
    let ok = std::fs::read(&p).map(|d| d == want).unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, &want).map_err(|e| format!("写入图标文件失败：{e}"))?;
    }
    Ok(p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// 注册表读写
// ---------------------------------------------------------------------------

/// 非提权执行 reg.exe（HKCU 写入不需要管理员权限，全程无黑框、无 UAC）
fn reg_run(args: &[&str]) -> Result<(), String> {
    let exe = format!(r"{}\System32\reg.exe", system_root());
    let out = Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("调用 reg.exe 失败：{e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if msg.is_empty() {
            "写入注册表失败".to_string()
        } else {
            format!("写入注册表失败：{msg}")
        });
    }
    Ok(())
}

/// 提权执行 reg.exe 并等待结束（仅在需要清理旧的 HKLM 残留时使用，会弹 UAC）
fn reg_elevated(args: &[&str]) -> Result<(), String> {
    let exe = format!(r"{}\System32\reg.exe", system_root());
    let verb = HSTRING::from("runas");
    let file = HSTRING::from(exe);
    let params = HSTRING::from(join_args(args));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: 0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| {
            if e.code().0 as u32 == ERROR_CANCELLED.0 {
                "已取消管理员授权，设置未生效".to_string()
            } else {
                format!("请求管理员权限失败：{e}")
            }
        })?;
        // SHELLEXECUTEINFOW 是 packed 结构体，直接取字段会产生未对齐引用（32 位下编译报错），
        // 用 addr_of! 按字节布局安全读出
        let h = std::ptr::addr_of!(info.hProcess).read();
        if !h.is_invalid() {
            WaitForSingleObject(h, INFINITE);
            let mut code = 0u32;
            let _ = GetExitCodeProcess(h, &mut code);
            let _ = CloseHandle(h);
            if code != 0 {
                return Err(format!("写入注册表失败（reg.exe 退出码 {code}）"));
            }
        }
    }
    Ok(())
}

/// 拼一条命令行（ShellExecuteExW 需要手写参数串，带空格的整体加引号）
fn join_args(args: &[&str]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') {
                format!("\"{a}\"")
            } else {
                (*a).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 拆出注册表根键（`HKCU\` / `HKLM\`）与子键路径
fn split_hive(key: &str) -> Option<(HKEY, &str)> {
    if let Some(rest) = key.strip_prefix("HKCU\\") {
        Some((HKEY_CURRENT_USER, rest))
    } else if let Some(rest) = key.strip_prefix("HKLM\\") {
        Some((HKEY_LOCAL_MACHINE, rest))
    } else {
        None
    }
}

/// 判断 `key` 下有没有名为 29 的值。
///
/// 走进程内的 `RegGetValueW`，**不再 spawn reg.exe**：这个查询在刷新设置页
/// (`get_shortcut_arrow_hidden`，同步命令 = 主线程) 和每次开关时都会跑，
/// 原来一次要创建 2~5 个 reg.exe 进程，主线程被卡住几百毫秒。
fn value_exists(key: &str) -> bool {
    let Some((hive, sub)) = split_hive(key) else {
        return false;
    };
    let sub_w: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
    let name_w: Vec<u16> = ARROW_VALUE.encode_utf16().chain(Some(0)).collect();
    unsafe {
        // 只要拿到数据大小即可判断存在，不需要读内容
        let mut size: u32 = 0;
        RegGetValueW(
            hive,
            PCWSTR(sub_w.as_ptr()),
            PCWSTR(name_w.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok()
    }
}

/// 当前是否已去掉小箭头（以系统级 HKLM 为准，兼容只写了用户级 HKCU 的旧状态）
pub fn is_arrow_hidden() -> bool {
    value_exists(HKLM_KEY) || value_exists(HKCU_KEY)
}

// ---------------------------------------------------------------------------
// 无感刷新
// ---------------------------------------------------------------------------

/// 需要刷新的目录（桌面、开始菜单、任务栏固定项）
fn shell_dirs() -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(u) = std::env::var("USERPROFILE") {
        v.push(format!(r"{u}\Desktop").into());
        v.push(format!(r"{u}\AppData\Roaming\Microsoft\Internet Explorer\Quick Launch").into());
        v.push(format!(r"{u}\AppData\Roaming\Microsoft\Windows\Start Menu\Programs").into());
    }
    if let Ok(p) = std::env::var("PUBLIC") {
        v.push(format!(r"{p}\Desktop").into());
        v.push(format!(r"{p}\Microsoft\Windows\Start Menu\Programs").into());
    }
    v
}

fn notify_path(event: SHCNE_ID, path: &str) {
    let mut w: Vec<u16> = path.encode_utf16().collect();
    w.push(0);
    unsafe {
        SHChangeNotify(
            event,
            SHCNF_PATHW,
            Some(w.as_ptr() as *const core::ffi::c_void),
            None,
        );
    }
}

/// 递归找快捷方式（限制层数，避免开始菜单很深时太慢）
fn collect_shortcuts(dir: &std::path::Path, depth: u32, out: &mut Vec<String>) {
    if depth == 0 || out.len() > 800 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_shortcuts(&p, depth - 1, out);
        } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            if ext.eq_ignore_ascii_case("lnk") || ext.eq_ignore_ascii_case("url") {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
}

/// 向 shell 发一轮刷新通知
fn notify_once() {
    unsafe {
        // 1) 关联变更：让 shell 作废并重建图标缓存。
        //    这里必须用 SHCNF_FLUSH（同步派发）而不是 SHCNF_FLUSHNOWAIT：
        //    后者不等待，shell 还没处理我们就走下一步了，刷新等于没做。
        SHChangeNotify(
            SHCNE_ASSOCCHANGED,
            SHCNF_IDLIST | SHCNF_FLUSH,
            None,
            None,
        );
    }
    // 2) 逐个快捷方式单独刷新 —— 只重绘这些项，桌面不会整体重画
    for dir in shell_dirs() {
        notify_path(SHCNE_UPDATEDIR, &dir.to_string_lossy());
        let mut links = Vec::new();
        collect_shortcuts(&dir, 3, &mut links);
        for l in links {
            notify_path(SHCNE_UPDATEITEM, &l);
        }
    }
}

/// 无感刷新桌面图标（不重启 explorer）。
///
/// 顺序很关键：通知作废缓存 → **等 ie4uinit 重建完** → 再通知一轮。
/// 之前是 spawn() 发起 ie4uinit 就往下走，通知比重建先到，刷新等于白做 ——
/// 这很可能就是"改了没反应、非得重启 explorer"的真正原因。
pub fn refresh_desktop_icons() {
    notify_once();

    // 重建图标缓存数据库：-show 是新版（Win10/11）的刷新开关，-ClearIconCache 兼容旧版
    let exe = format!(r"{}\System32\ie4uinit.exe", system_root());
    if std::path::Path::new(&exe).exists() {
        for arg in ["-show", "-ClearIconCache"] {
            // output() 会等进程跑完再返回（spawn() 不会）
            let _ = Command::new(&exe)
                .arg(arg)
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }
    }

    // 缓存已经是新的了，再通知一轮：shell 会按新的 Shell Icons 重新合成图标
    notify_once();

    // 400ms 后再补一轮，覆盖 explorer 正忙的情况
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(400));
        notify_once();
    });
}

// ---------------------------------------------------------------------------
// 对外：开关 + 兜底
// ---------------------------------------------------------------------------

/// 写 / 删「系统级」HKLM 下的 29 值。
/// 优先直接写（进程已提权时无任何弹窗），失败再请求一次 UAC。
/// 返回 Err 只在两种情况下发生：被拒绝授权、或者两次都写失败。
fn write_hklm(icon: Option<&str>) -> Result<(), String> {
    // 本来就没有这个值就不必删 —— 否则会白白弹一次 UAC
    if icon.is_none() && !value_exists(HKLM_KEY) {
        return Ok(());
    }
    let args: Vec<&str> = match icon {
        Some(p) => vec!["add", HKLM_KEY, "/v", ARROW_VALUE, "/t", "REG_SZ", "/d", p, "/f"],
        None => vec!["delete", HKLM_KEY, "/v", ARROW_VALUE, "/f"],
    };
    if reg_run(&args).is_ok() {
        return Ok(());
    }
    reg_elevated(&args)
}

/// 隐藏（true）或恢复（false）快捷方式小箭头。
///
/// 关键点一：**以系统级 HKLM 为准**。实测（Win11 26H1 / Build 28000）explorer 会忽略
/// 当前用户级（HKCU）的 `Shell Icons\29` 覆盖 —— 这正是"注册表明明写进去了、箭头却
/// 还在"的原因。HKCU 一起写只是为了兼容那些只认用户级设置的旧版本，不影响结果。
/// HKLM 需要管理员权限：进程已提权则直接写（无弹窗），否则会请求一次 UAC。
///
/// 关键点二：写完**一定要重建图标缓存**（作废 → ie4uinit 重建 → 再通知一轮），
/// 否则 shell 会继续复用 `iconcache_*.db` 里那份带箭头的旧图。
/// 这条路径**不重启 explorer**（重启会闪一下黑屏）；个别位置若仍是旧的，
/// 再用 [`force_apply`] 强制重启一次。
pub fn set_arrow_hidden(app: &AppHandle, hide: bool) -> Result<(), String> {
    let icon = if hide {
        Some(ensure_blank_ico()?)
    } else {
        None
    };

    // 两条路径**互不短路**：之前 HKLM 删除失败会 `?` 直接返回，
    // HKCU 的残留根本没被清掉 —— 表现就是"取消勾选后箭头还是回不来"。
    //
    // 用户级（无需管理员权限，纯兼容旧系统）
    let _ = match &icon {
        Some(p) => reg_run(&[
            "add",
            HKCU_KEY,
            "/v",
            ARROW_VALUE,
            "/t",
            "REG_SZ",
            "/d",
            p,
            "/f",
        ]),
        None => reg_run(&["delete", HKCU_KEY, "/v", ARROW_VALUE, "/f"]),
    };

    // 系统级：本机真正生效的位置（需要管理员权限，失败会请求一次 UAC）
    let hklm = write_hklm(icon.as_deref());

    if hide {
        // 软刷新：作废 + 重建图标缓存并让 shell 重新合成，**不重启 explorer**（不闪黑屏）
        refresh_desktop_icons();
    } else {
        // 恢复箭头必须重启 explorer：explorer 只在启动时读取一次 `Shell Icons\29`
        // 并把叠加索引缓存在进程里，之后无论怎么 SHChangeNotify / ie4uinit 都不会重读 ——
        // 这正是「注册表删掉了、箭头却回不来」的根因。清缓存 + 重启是唯一可靠的办法。
        restart_explorer(app);
        // 本进程的 shell32 同样缓存着叠加索引，通知一轮让它作废，
        // 否则分区里重新提取的图标仍是那张透明箭头图
        notify_once();
    }

    // 以注册表实际状态为准回报结果，不谎报成功
    if value_exists(HKLM_KEY) != hide {
        return Err(match hklm {
            Err(e) => format!("系统级设置未生效（需要管理员权限，请在 UAC 弹窗点“是”）：{e}"),
            Ok(()) => "系统级设置未生效，请重试或点“强制生效”".to_string(),
        });
    }
    Ok(())
}

/// 图标缓存数据库文件（Win8 之后在 Explorer 目录下的 iconcache_*.db，更早的是 IconCache.db）
fn icon_cache_files() -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = Vec::new();
    let Ok(la) = std::env::var("LOCALAPPDATA") else {
        return v;
    };
    let base = std::path::Path::new(&la);
    v.push(base.join("IconCache.db"));
    let dir = base.join("Microsoft").join("Windows").join("Explorer");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("iconcache") && name.ends_with(".db") {
                v.push(e.path());
            }
        }
    }
    v
}

/// 删除图标缓存文件。必须在 explorer 退出后调用 —— explorer 在跑的时候这些文件被占用，
/// 删不掉，于是它会一直用缓存里的旧箭头图（这正是"重启了也没用"的原因）。
fn purge_icon_cache() {
    for p in icon_cache_files() {
        if !p.exists() {
            continue;
        }
        // 只读属性会让删除失败，先摘掉
        if let Ok(md) = std::fs::metadata(&p) {
            let mut perm = md.permissions();
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(&p, perm);
        }
        let _ = std::fs::remove_file(&p);
    }
}

/// explorer.exe 是否还在运行
fn explorer_running() -> bool {
    // 进程内快照，不再 `tasklist`：explorer 重启时被放进轮询循环调用，
    // 原来一次重启最多能创建 130 个 tasklist 进程（每个都要枚举整张进程表）。
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut e = PROCESSENTRY32W::default();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&e.szExeFile);
                if name.trim_end_matches('\0').eq_ignore_ascii_case("explorer.exe") {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// 重启资源管理器：结束 explorer -> 等它真正退出 -> 删掉图标缓存 -> 立刻重新拉起。
///
/// 缓存必须在 explorer 退出后清掉，否则它会继续复用旧图；同时"没桌面"的时间要压到
/// 最短 —— 轮询间隔 30ms、退出后立刻拉新进程，尽量只留一次极短的闪动。
pub fn restart_explorer(app: &AppHandle) {
    // 组件/分区挂在 explorer 的桌面窗口(Progman)下面，explorer 退出会销毁 Progman，
    // **子窗口会被父窗口连带销毁** —— 必须先摘下来，explorer 起来之后再挂回去。
    crate::boxwin::detach_from_desktop_all(app);

    let _ = Command::new("taskkill")
        .args(["/f", "/im", "explorer.exe"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    for _ in 0..80 {
        if !explorer_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    purge_icon_cache();
    // 立刻拉起，不再额外 sleep，缩短黑屏
    let _ = Command::new("explorer.exe").spawn();
    for i in 0..50 {
        if explorer_running() {
            break;
        }
        // 超过 ~1.2s 还没起来，再补拉一次，避免桌面回不来
        if i == 30 {
            let _ = Command::new("explorer.exe").spawn();
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    // 等新的桌面窗口(Progman)建好再挂回去，否则找不到父窗口，分区就回不到桌面层了
    std::thread::sleep(Duration::from_millis(800));
    crate::boxwin::attach_to_desktop_all(app);
}

/// 强制生效：HKCU + HKLM 双写（HKLM 需要一次管理员授权）、重建图标缓存、重启资源管理器。
///
/// 为什么要写 HKLM：少数 Win11 版本会忽略当前用户（HKCU）下的 Shell Icons 覆盖，
/// 只认系统级（HKLM）的设置；两处都写上，覆盖面最大。
/// 返回一段给界面展示的说明文字。
pub fn force_apply(app: &AppHandle, hide: bool) -> Result<String, String> {
    let mut notes: Vec<String> = Vec::new();

    if hide {
        let icon = ensure_blank_ico()?;
        // 系统级是唯一真正生效的位置：写不进就直接报错，
        // 不要"静默降级"成只写 HKCU —— 那样界面显示成功、桌面却毫无变化
        write_hklm(Some(&icon))?;
        notes.push("已写入系统级设置".to_string());
        let _ = reg_run(&[
            "add",
            HKCU_KEY,
            "/v",
            ARROW_VALUE,
            "/t",
            "REG_SZ",
            "/d",
            &icon,
            "/f",
        ]);
        notes.push("已写入当前用户设置".to_string());
    } else {
        if value_exists(HKLM_KEY) {
            write_hklm(None)?;
        }
        if value_exists(HKCU_KEY) {
            let _ = reg_run(&["delete", HKCU_KEY, "/v", ARROW_VALUE, "/f"]);
        }
        notes.push("已去掉注册表覆盖".to_string());
    }

    restart_explorer(app);
    notes.push("已重建图标缓存并重启资源管理器".to_string());
    Ok(notes.join("；"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 透明图标结构自检：目录项偏移/长度连续、掩码按 4 字节对齐、且整张图彻底不可见
    /// （像素 alpha 全 0，AND 掩码全 1 —— 两者任一写成"不透明"都会在图标上留下方块）
    #[test]
    fn blank_ico_layout_is_consistent() {
        let b = blank_ico_bytes();
        assert_eq!(b.len(), blank_ico_total());
        assert_eq!(u16::from_le_bytes([b[4], b[5]]) as usize, ICO_SIZES.len());

        let mut expect_off = 6 + 16 * ICO_SIZES.len();
        for (i, &d) in ICO_SIZES.iter().enumerate() {
            let e = 6 + i * 16;
            assert_eq!(b[e] as u32, d); // width
            assert_eq!(b[e + 1] as u32, d); // height
            assert_eq!(u16::from_le_bytes([b[e + 4], b[e + 5]]), 1); // planes
            assert_eq!(u16::from_le_bytes([b[e + 6], b[e + 7]]), 32); // bitCount
            let len = u32::from_le_bytes([b[e + 8], b[e + 9], b[e + 10], b[e + 11]]) as usize;
            let off = u32::from_le_bytes([b[e + 12], b[e + 13], b[e + 14], b[e + 15]]) as usize;
            assert_eq!(len, ico_image_len(d));
            assert_eq!(off, expect_off);
            assert!(off + len <= b.len());
            expect_off += len;

            // 像素 BGRA：RGB 全 0、alpha 全 1。
            // alpha 绝不能是 0 —— 整幅 alpha 全 0 会被当成空图退化成不透明黑块
            let px = (d * d * 4) as usize;
            let pxs = &b[off + 40..off + 40 + px];
            assert!(pxs.iter().step_by(4).all(|&x| x == 0)); // B
            assert!(pxs.iter().skip(1).step_by(4).all(|&x| x == 0)); // G
            assert!(pxs.iter().skip(2).step_by(4).all(|&x| x == 0)); // R
            assert!(pxs.iter().skip(3).step_by(4).all(|&x| x == 1)); // A
            // AND 掩码全 1 = 全透明（给不看 alpha 的老渲染路径兜底）
            let mask = ico_mask_len(d);
            assert!(b[off + 40 + px..off + 40 + px + mask]
                .iter()
                .all(|&x| x == 0xFF));
        }
        assert_eq!(expect_off, b.len());

        // 掩码行按 4 字节对齐：这些尺寸都比"宽*高/8"更大
        assert_eq!(ico_mask_len(16), 64);
        assert_eq!(ico_mask_len(20), 80);
        assert_eq!(ico_mask_len(24), 96);
        assert_eq!(ico_mask_len(32), 128);
        assert_eq!(ico_mask_len(40), 320);
        assert_eq!(ico_mask_len(48), 384);
        assert_eq!(ico_mask_len(64), 512);
        assert_eq!(ico_mask_len(128), 2048);
    }
}
