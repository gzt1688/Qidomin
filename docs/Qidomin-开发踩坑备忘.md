# Qidomin 项目长期备忘

> 只留「不知道就会踩坑」的硬约束。过程与历史见 `.workbuddy/memory/日 志`、`docs/`。
> 完整旧版备份：`.workbuddy/tmp/MEMORY-full-backup-20260914.md`

## 编译 / 打包
- **交付件必须 `npm run tauri build`**（= `打包.bat`）→ `src-tauri\target\release\qidomin.exe`（便携）+ `...\bundle\nsis\*-setup.exe`。
- **禁止 `cargo build --release` 出交付件**：Tauri 靠 `custom-protocol` 特性（只有 CLI 会加）判定生产模式；直接 cargo 编出的是**开发模式** exe —— 前端不嵌入、运行时连 `http://localhost:1420`，**所有窗口显示「无法访问此页面」**（看着像"面板全黑"）。
- 自检：`grep -a -o "assets/" qidomin.exe | wc -l`（正常 ≈17，0 = 编错）。⚠️ **别在 exe 里搜前端文案验证改动** —— 资源**内容**被 brotli 压缩（只有**文件名**不压缩）。正确做法：`ls dist/assets/` 取新 chunk 名（如 `App-XXXX.js`），再 `grep -a -c "App-XXXX.js" qidomin.exe`。
- NSIS 最后一步偶报 `os error 32（文件被占用）`：**此时 exe 已编好**，直接部署，不必重跑 4 分钟。
- 构建前先 `taskkill /im qidomin.exe`（否则 exe 被占用）。只用 `cargo check` / `cargo test` 做代码级检查，别顺手 `cargo build --release`（会覆盖打包.bat 的输出路径）。

## 运行环境
- bash 必须先 `export PATH="/c/Windows/System32:/c/Windows:/usr/bin:/bin:$PATH"`；npm 在 `D:\Program Files\nodejs\npm.cmd`，cargo 在 `~/.cargo/bin`。
- ⚠️ 这条 PATH **结尾必须带 `:$PATH`**；只写固定几段会挤掉 nodejs → 构建 3 秒内失败、只报一句 `'"node"' 不是内部或外部命令`。
- 日志 `%APPDATA%\com.qidomin.app\qidomin.log`（目录名硬编码，identifier 改了也不变）；单实例锁 `instance.lock`、退出守卫 `qidomin_guard.exe` 同目录。
- **数据库两套目录别读错**：`%APPDATA%\com.qidomin.app\qidomin.db`（旧标识，停更）vs **`%APPDATA%\com.qidomin.desktop\qidomin.db`（当前在用，WAL）**。判断依据 = mtime 是否随当前进程更新；读 WAL 库要连 `-wal`/`-shm` 一起复制。
- WebView2 用户目录 `%LOCALAPPDATA%\com.qidomin.desktop\EBWebView`。
- 普通分区实体文件夹 = 桌面\<分区名>（HIDDEN|SYSTEM），退出归还桌面；组件窗口 label `notes`/`stocks`/`huangli`/`poetry`，分区 `box-<id>`。

## 桌面常驻（显示桌面免疫）
- 方案 = **顶层窗口 + 防最小化子类过程 + 桌面层守护线程**（不挂 Progman 子窗口：那会让 WebView2 逐像素透明在部分机器失效 → 面板整块黑）。
- **Win11 实测（26H1）**：「显示桌面」**既不最小化也不隐藏**，而是把 Progman 抬到几乎所有普通窗口之上 → 面板被整块盖住（`IsWindowVisible=1`、`IsIconic=0`、CLOAKED=0、零日志、用户感觉=被最小化）。**消息层拦截对这条路径无效**。
- 真解 = `boxwin.rs` 桌面层守护：枚举 z 序，把受保护窗口插到「桌面组」（Progman + 紧邻其上连续 WorkerW）顶端之上。日志 `desktop-keeper: 把 N 个窗口抬回桌面组之上`。
- **守护必须够快**：120ms 轮询 = 被盖住 36ms（肉眼可见闪）；事件驱动 = **1.94ms（亚帧，不可见）**。三路唤醒：① 子类在 `WM_WINDOWPOSCHANGING` 里摘 `SWP_HIDEWINDOW`/改写 `HWND_BOTTOM` 时调 `kick_desktop_keeper_burst()`（之后 600ms 转 4ms 高频）；② `SetWinEventHook(EVENT_OBJECT_REORDER + EVENT_SYSTEM_MINIMIZESTART/END)` —— **必须泵消息**，故线程用 `MsgWaitForMultipleObjectsEx`（wake mask 要显式含 `QS_EVENT(0x2000)`，它不在 `QS_ALLINPUT` 里）+ `PeekMessage/DispatchMessage`；③ 高精度可等待定时器 120ms 兜底。
- ⚠️ **`DeferWindowPos` 不支持 `SWP_NOSENDCHANGING`** → 返回 NULL、`err=87`（`SetWindowPos` 支持）。曾因合批失败只 `break`、`fixed_z` 保持 0 → 既不修复也不打日志 → 面板再也回不来。**守护/兜底动作绝不能静默失败**：现在失败降级为逐个 `SetWindowPos` 并打日志，`desktop_keeper_tick` 外包 `catch_unwind`。
- **`SetWindowPos(hwnd, hWndInsertAfter, …)` 是"插到该窗口下面"**：要让窗口在 X 之上必须传 `GetWindow(X, GW_HWNDPREV)`。
- 主动隐藏（托盘最小化、收起分区、关组件）必须走 `boxwin::hide_win()`/`hide_window()`（登记带时间戳的一次性许可）；守护会清掉「标记 >1.5s 却仍可见」的陈旧许可。
- 辅助拦截保留：`WM_WINDOWPOSCHANGING`、`WM_SHOWWINDOW(wp=0)`、`WM_SIZE(SIZE_MINIMIZED)`。
- **守护必须双向核验（2026-09-15 修「新开软件压在面板下」）**：只保证「面板在桌面组之上」不够 —— 显示桌面后外壳把 Progman 沉回底部时，**面板留在被抬高过的位置**，之后新开的软件窗口全压在面板下面。现在 `keeper_scan_cb` 连「合格的普通窗口」一起记（可见+未最小化+非 TOPMOST+未 cloak+非本进程，`is_qualifying_normal_window()`），tick 发现「桌面组之上有普通窗口却在面板之下」→ 面板整体沉回桌面组之上。显示桌面进行中不误触发（那时普通窗口全在桌面组之下）。日志：`…N 个可见普通窗口压在面板上，已整体沉回`；心跳字段「普通窗压板」。

## 分区窗口：几何 / 锁定 / 幽灵窗口
- 几何存在设置键 `boxWindows`（JSON 数组 `{id,x,y,w,h,locked,visible,…}`）；分区窗口 = 独立顶层无边框 WebView2 窗口，label `box-<id>`。
- `locked` = 不能拖 + 不能改尺寸 + 原生禁拉伸（`save_box_window_cfg` 同步 `set_resizable(!locked)`）→ 解锁入口不可达就彻底动不了。
- **锁定死局（已修）**：分区拉窄后标题栏总宽超出、右侧锁定/⋮ 被裁掉 → 点不到解锁。两防线：① 右键菜单加锁定/解锁（独立置顶窗口，永不被裁）；② `BoxWindow.tsx` 用 `ResizeObserver` 加 `head-tight`(<330)/`head-tight-2`(<280)，按「项数→搜索框」依次收起。
- **`import_layout` 是幽灵窗口温床（已修）**：它清空重建 `boxes`（id 全换新），曾经只开新窗口不关旧的 → 旧窗口留在桌面、注册在过期 label、库里无记录，还被守护保护 → 出现两个同名窗口。现在先记旧 id，事务提交后逐个 `destroy_box_window`。
- **导入数值必须夹下限**：备份里 `w/h` 缺失被 serde 补 0，`w.max(140)`/`h.max(100)` → **140×100 + 屏幕左上角**，若是 locked 就是"卡死在左上角动不了"。现在 `box_size_from_cfg()` 把"小于下限"当"没配置过" → 默认 330×250、重新安置并回写。**教训：`max(min)` 会把 0 变成"看起来正常"的碎片。**
- 兜底 `boxwin::sweep_ghost_box_windows()`（`open_all_box_windows` 末尾）销毁 label 属于已删分区的窗口。幽灵窗口只在当次进程内存在（重启即消失）。
- 诊断关键字：`分区 N 的窗口尺寸配置不合法…重新安置`、`导入布局：关闭旧分区窗口 X（id=N）`、`清理幽灵分区窗口：…`。

## 拖放落点：三条通道必须收口成一个赢家
- 同一落点会从三条通道各送一次（= "一个文件进了两个分区"）：① 我们的 `IDropTarget::Drop`（`shelldrop.rs`）；② **WebView2 内建拖放 → wry → tao `WindowEvent::DragDrop::Drop`**（`RevokeDragDrop` 够不着，**关不掉**）；③ OLE 没给 Drop 时鼠标钩子兜底（抬起后 400ms）。
- 统一入口 `shelldrop::admit(app, box_id, screen, via)`，两条规则缺一不可：
  - **只有光标下 z 序最上层的分区能收** —— `boxwin::box_at_point()`（`WindowFromPoint` + `GA_ROOT`）。**不能用矩形包含**（忽略遮挡：被压在背后的分区也会收下）。
  - **一次拖动只放行一个分区** —— `DOWN_GEN`（`WM_LBUTTONDOWN` 钩子里 +1）分代 + `CLAIMED_GEN` CAS 抢占，1.5s 超时兜底；`PendingDrop` 记 `down_gen`，代次变了即作废。
- `route_drop()` 是"真实文件"落点唯一入口（空载荷直接 return，不占名额）；**被拒绝时绝不能写 `LAST_DROP_MS`**（否则兜底通道以为有人接住 → 整次拖放没人处理）。
- 历史坑：曾写过一个 `claim_drop` 去重，**从未被调用**（死代码）→ 设闸门要确认真在调用链上。
- 日志：`shelldrop: 落点（IDropTarget|tao事件|兜底）「分区名」，N 项`、`…已由「Y」受领`、`…光标(x,y)下不是它`。
- `DragEnter … 命中其它窗口 hwnd=` **是假警报**：WebView2 客户区被自己的 `Chrome_RenderWidgetHostHWND`/`TAURI_DRAG_RESIZE_BORDERS` 子窗口盖住，比对必须取 `GA_ROOT`。

## 搜索 / 文件索引（FTS5 trigram）
- 索引表 `files_fts`（FTS5，`tokenize='trigram case_sensitive 0'`），`scope` = `common` 或盘符；`usn.rs` 实时维护 NTFS 盘，`search.rs` 15 分钟兜底重扫。
- ⚠️ **trigram 至少要 3 个字符才切得出三元组**：短于 3 个字符的查询 `MATCH` **恒为 0 条**。中文常态输入恰恰是 1~2 字（"微信" / "火绒" / "淘宝"）→ 曾经表现为"**中文搜索整个无效**"（打 3 个字以上才好）。修法：`< 3` 字符走 `name LIKE ?%…% ESCAPE '\'` 全表扫描兜底（阈值 = `Db::TRIGRAM_MIN_CHARS`）。
- **判长度必须 `query.chars().count()`**：2 个汉字 = 6 字节，用 `len()` 会被误判成 ≥3 → 又落回 MATCH → 又是 0 条。
- **两条路结果语义完全一致**（117 个随机 3 字窗口实测 MATCH vs LIKE 差异 0），但 trigram 快 ~125×（0.6ms vs 71ms / 17.4 万行库）→ **别图省事把 ≥3 字符也改成 LIKE**。
- 实测（17.4 万行 / 131MB 库）：LIKE 全表扫描 ≈ **40ms**（含 scope + 类型过滤 + `GROUP BY path`），可接受。**FTS5 虚拟表上的 `name LIKE` 走不了 trigram 索引**（`scope IN` 是 UNINDEXED 列约束，EXPLAIN 恒为 `VIRTUAL TABLE INDEX 0:`）→ 短查询就是老老实实全表扫。
- LIKE 通配符必须转义（`esc_like`，配 `ESCAPE '\'`）：不转义时搜 `%` / `_` 会命中一切。
- 两段式语义：先前缀 `x%`，不足 limit 再子串 `%x%`（应用层按 path 去重）→ "以查询开头"的排前面。空 `exts` 列表 = 该大类没勾任何后缀 = 直接无结果。
- 转义与分流**统一在 `db::search_files`**（`query` 传原始未转义串，内部用 `esc_fts` / `esc_like`）；`commands::search_files` 不再自己拼 MATCH 表达式。
- 回归测试 `db::tests::short_query_like_fallback`（1/2/3 字中文 + 前缀优先 + 类型过滤 + 通配符转义）、`escape_helpers`。
- 诊断脚本 `.workbuddy/tmp/dbprobe/probe*.py`：读索引前**必须把 `qidomin.db` + `-wal` + `-shm` 一起复制**到临时目录，否则读到的是旧快照（当前库 `%APPDATA%\com.qidomin.desktop\qidomin.db`）。

## 便签（notes）
- 便签行 = 一条 `Note`；**末尾始终留一行空白**（`ensureTail`），失焦清中间空行。
- 窗口配置键 `noteWindow`（`NoteWindowCfg`，Rust `db.rs` + TS `types.ts` 两处）。**加字段必须 `#[serde(default = "…")]`**，否则老配置反序列化失败 → 用户设置全被重置。
- **算式自动计算**：`src/calc.ts`，自写递归下降解析器（**不用 eval / new Function**）。整行以 `=`/`＝` 结尾 → 只算**最后一个等号之后**那段（可连算 `6+3=9+1=10`）；那段须全为数字/运算符/括号且**至少一个二元运算符**（否则 `123=` 会被当成数字）。除零 / >1e15 / 括号不配对 → 不算。唯一入口 `calcOnType(value, id, memo)`；`memo={id,input,output}` 保证结果删得掉、正文一变即作废（**别用"只在变长时算"**，那会堵死全选替换）。开关在右键菜单（键 `calc`，缺省开）。用例 `.workbuddy/tools/calc-test.mjs`（50 条）。
- 组件右键菜单是**独立置顶窗口**（`WidgetMenuWindow.tsx`）。**别放长文字 label**（会折行撑高 → 菜单显示后调高度 = 肉眼可见的闪）。

## 股票行情（`stock.rs`）
- **主源 = 腾讯 `qt.gtimg.cn`（一次批量），备源 = 东财 `stock/get`（逐只并发）**，两者都不通才报错。
- ⚠️ **东财 2026-09 下线了批量接口 `ulist.np/get`**：请求被服务端**直接断连**（`RemoteDisconnected`），换 `ut`、换子域（`82.push2`）、带 Cookie、改 POST **一律无效**；同一台机器上 `stock/get`、`clist/get`、`quote.eastmoney.com` 首页却全部正常 → 别再试图"调参救活"它。只剩单只接口，逐只请求 = 每轮请求数 × 自选股只数（慢，且更容易被风控），所以把批量能力交给腾讯做主源。
- ⚠️ **ureq 的 `native-tls` 必须显式装配，否则 https 全挂**：ureq 2.x 里 `native-tls` 只是个「适配器」feature，**不会**成为默认 TLS 后端（源码注释原文 *"it is never picked up as a default"*），默认只认 `tls`（rustls）那个 feature，而我们故意没开（rustls 会对不发 `close_notify` 的行情服务器判错）。不显式装配的话所有 https 请求直接失败：
  `Unknown Scheme: cannot make HTTPS request because no TLS backend is configured`
  正确写法：`AgentBuilder::new().timeout(..).tls_connector(Arc::new(ureq::native_tls::TlsConnector::new()?))`。
  **排查提示**：这条报错不含 "网络" 字样，经 `short_err()` 截断后只剩一个 URL，极易误判成"对方服务器把我断了"——先看后缀是不是 `Unknown Scheme`。
- 腾讯返回体是 **GBK**（用 `encoding_rs::GBK` 解，直接 UTF-8 解会得到乱码名称），`~` 分隔，一只一行 `v_<代码>="…";`。**A股 / 港股 / 美股字段索引完全一致**：[1] 名称 [2] 代码 [3] 最新价 [4] 昨收 [5] 今开 [31] 涨跌额 [32] 涨跌幅% [33] 最高 [34] 最低；只是数组长度不同（A股 88 项 / 港股 78 / 美股 73），不足 35 项视为残缺跳过。腾讯**不返回**它不认识的代码（没有对应行）→ 解析结果为空时应降级到东财再试一次（否则会静默丢自选股）。
- secid ↔ 腾讯代码换算：`1.` → `sh`、`0.` → `sz`、`116.` → `hk`、`105.` → `us`。**解析后必须用调用方传入的 secid 原值回填**，不要按接口返回的代码重拼 —— 前端拿 `q.secid` 当 map key 匹配，拼错那一只就整行空白（港股 `116.01810` 接口回 `01810`）。
- 回归测试：`cargo test --lib stock::tests`（纯函数，含 GBK 解码与字段索引）；真实联网用例 `cargo test --lib stock::tests::live -- --ignored --nocapture`。探测脚本在 `.workbuddy/tmp/stockprobe/`。

## 鼠标快捷键（`mousehotkey.rs`）
- `WH_MOUSE_LL` + `WH_KEYBOARD_LL`（独立线程 + 消息泵）；键 `mouseHotkeys`（`MouseBinding[]`）+ 总开关 `mouseHotkeysEnabled`；识别信号 6 秒经 `mouse-captured` 回传（Esc 取消）；动作表 `MOUSE_ACTIONS`。
- 默认两条（`commands::default_mouse_hotkeys`）：`middle` + `spaceAfter` → 快速搜索；`middle2`（双击中键）→ 显示/隐藏全部分区。
- ⚠️ **判「另一个键还按着」必须用自己记的物理状态（`SPACE_DOWN` / `MOUSE_DOWN`），不能用 `GetAsyncKeyState`**：组合成立后要把空格连抬起一起吞，而被吞的抬起不写进系统按键表 → 空格在系统眼里"永远按着"。状态更新**必须放在所有提前 return 之前**。
- **空格组合的触发条件只有一条：两个键同时按着**。`SPACE_ARMED`/`SPACE_REPLAYED`/`SPACE_MUTED` 只管"这一下空格怎么投递"（先扣下，没凑成组合再原样补发），**不是第二套判据**。
- 空格按着时的中键不参与「双击中键」判定（把 `MID_LAST_DOWN_MS` 清零）。
- 自己 `SendInput` 的事件一律放行（`LLKHF_INJECTED`）—— 否则补发的空格会被自己再吞一次。**以后往 `send_keys` 加键要重新检查这条。**
- 支持按键：中键 / 双击中键 / 侧键1(X1) / 侧键2(X2)。鼠标专用键（罗技 G6~G9 等）Windows 收不到事件，本模块不管。

## 工作方式约定
- **并行 Edit 同一文件会互相覆盖**（两个 edit 都报"成功"但只有一个生效）→ 同文件多处改动**逐个串行**；大段删除用 python 按行号删 + `assert` 校验边界。
- `EnumWindows` 枚举顺序即 z 序（Python ctypes 可用；PowerShell `Add-Type` 被沙箱拦）。
- 判断"屏幕上是否多了一个窗口"：ctypes `EnumWindows` 按 pid 过滤 + 打印 `hwnd/title/class/GetWindowRect/IsWindowVisible/IsIconic/exstyle/style`，再 `EnumChildWindows` 看有无 `Chrome_WidgetWin_1` 子窗口（有 = 真窗口）。同名窗口出现两次 + 库里分区数对不上 = 幽灵窗口。
- **智能体无法自己启动 Qidomin 做验证**：启动时会 spawn `reg.exe`，而 `reg.exe` / `schtasks.exe` 在沙箱黑名单 → 实例卡死或直接死（`CREATE_BREAKAWAY_FROM_JOB` 也被拒）。**验证要请用户双击 `D:\qidomin.exe`**，我再读日志。
- 可复用诊断脚本（`.workbuddy/tmp/`）：`zflash2.py`（量"显示桌面"时被盖住的毫秒数）、`dwprobe.py`（隔离验证 z 序 API）、`zprobe.py`（列顶层窗口 z 序/矩形/样式 + 两两重叠，排查落点归属必用）。量测只统计 `IsWindowVisible` 为真的窗口。
- 日志关键字：`desktop-keeper: 守护线程已启动`、`…心跳 枚举=.. 桌面组顶idx=.. 目标=..`（5 分钟一条）、`…把 N 个窗口抬回（触发后 Xms）`（≤10ms 正常）。

## 历史（已废弃，勿再投入）
- **罗技 G 键 / HID++ 直连：代码 2026-09-15 已全删**（`logitech.rs` 整个模块、`commands::logitech_probe`、`mod logitech`、设置页「罗技鼠标 G 键」区块 + 前端 `LogiProbe`/`logitechProbe()`、`mousehotkey.rs` 的 F13~F24 支持、Cargo.toml 两个只为探针加的 Win32 feature）。
  - 结论：G502 Hero 主机侧**没有** `0x1B04`（FeatureSet 权威枚举 18 项）；「G HUB 一次性映射成 F13~F24」**实测失败**（映射写 `_m1`，设备活动储存位 = 4）。
  - 详情见 `docs/罗技G键-HID++直连研究结论.md`；协议与踩坑留在技能 `~/.workbuddy/skills/logitech-hidpp/`（含探针源码、`ghub_revert.py`、G HUB 配置格式），GUI 自动化工具在 `win-gui-ctypes/`。
  - 若将来做方案 A（直读写 `0x8100` 板载配置）：先"读出 → 原样写回 → 校验无变化"并备份原始配置，且先取得用户明确同意。

## 用户环境
- 2560×1440、缩放 100%；鼠标 G502 HERO 有线（`046D:C08B`）；桌面常开 JDSoft SurfMill / Rhino / 千牛 等窗口 → **截屏一律裁剪到目标窗口**。
- 部署位置 `D:\qidomin.exe`（旧版留 `.bak` / `.bak2`）。
