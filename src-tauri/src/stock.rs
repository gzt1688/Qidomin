//! 桌面股票面板组件：自选股实时行情 + 独立透明窗口。
//!
//! 行情来源为东方财富公开行情接口（UTF-8 JSON），一次可批量查询多只股票：
//! 请求放在后端而不是前端 WebView，一是绕开跨域限制，二是可以把
//! "用户输入的代码" 统一翻译成接口需要的 secid（如 600000 → 1.600000）。

use crate::boxwin;
use crate::db::{Db, StockWindowCfg};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

pub const STOCK_LABEL: &str = "stocks";
/// 组件开关：设置页“组件 → 股票”的勾选状态
const ENABLED_KEY: &str = "stocksEnabled";
/// 开关变化事件名（发给设置窗口，用于同步勾选框）
const ENABLED_EVENT: &str = "stocks-enabled-changed";
/// 面板可见性事件（发给股票面板本身）：false 时前端停掉轮询定时器
pub const PAUSE_EVENT: &str = "stocks-paused";

static LAST_SAVE: Mutex<Option<Instant>> = Mutex::new(None);
static DRAGGING: AtomicBool = AtomicBool::new(false);

// ---------- 行情 ----------

/// 行情入口按顺序回退。实测部分网络下 push2.eastmoney.com 会被直接断连
/// （rustls 报 “peer closed connection without sending TLS close_notify”），
/// push2delay 是同一份行情数据的另一个入口，可用性明显更好。
/// 沪深（行情号 0/1）不在 push2delay 的延迟市场列表里，所以拿到的是同一份实时数据。
const QUOTE_HOSTS: [&str; 3] = [
    "https://push2.eastmoney.com",
    "https://push2delay.eastmoney.com",
    "http://push2delay.eastmoney.com",
];
/// 单只行情接口。
///
/// **不要改回 `ulist.np/get`**：2026-09 起东财把它下线了 —— 服务端收到请求后
/// 直接关连接（`RemoteDisconnected`），换 ut / 换子域 / 带 Cookie / 改 POST
/// 全都一样，跟本机网络无关（同一台机器上 `stock/get`、`clist/get` 都正常）。
/// 代价是不能一次批量查，必须逐只请求，所以 `fetch_quotes` 用并发来补回速度。
const QUOTE_PATH: &str = "/api/qt/stock/get";
/// `fltt=2` 下这些字段直接是小数（不是放大 100 倍的整数），原先的 ÷100 换算不需要：
/// f43 最新价 f44 最高 f45 最低 f46 今开 f57 代码 f58 名称 f60 昨收 f169 涨跌额 f170 涨跌幅%
const QUOTE_FIELDS: &str = "f57,f58,f43,f44,f45,f46,f60,f169,f170";
const QUOTE_UT: &str = "bd1d9ddb04089700cf9c27f6f7426281";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

// ---------- 腾讯行情（主源） ----------
//
// 为什么主源换成腾讯：东财 2026-09 下线了批量接口 `ulist.np/get`，只剩单只
// `stock/get`；逐只请求等于把每轮刷新的请求数放大成「自选股只数」倍，既慢
// 又更容易被风控拦掉。腾讯 `qt.gtimg.cn` 一次请求就能拿回整张自选股表
// （实测 20 只约 176ms），请求数最少。东财降级为兜底源。
const TX_HOST: &str = "https://qt.gtimg.cn";
/// 单次批量上限。腾讯没公布数字，实测 20 只正常，取 60 作保守值。
const TX_BATCH: usize = 60;

/// 腾讯返回体的字段索引（`~` 分隔）。A股 / 港股 / 美股这几项位置完全一致，
/// 只是数组长度不同（A股 88 项 / 港股 78 / 美股 73），所以按索引取即可：
/// [1] 名称 [2] 代码 [3] 最新价 [4] 昨收 [5] 今开
/// [31] 涨跌额 [32] 涨跌幅% [33] 最高 [34] 最低
const TX_NAME: usize = 1;
const TX_CODE: usize = 2;
const TX_PRICE: usize = 3;
const TX_PREV_CLOSE: usize = 4;
const TX_OPEN: usize = 5;
const TX_CHANGE: usize = 31;
const TX_CHANGE_PCT: usize = 32;
const TX_HIGH: usize = 33;
const TX_LOW: usize = 34;
/// 少于这么多字段说明这行残缺，跳过
const TX_MIN_FIELDS: usize = 35;

/// 上一次成功入口（下标 + 时间）：优先复用，免得每轮刷新都先撞一次失败。
/// 只保留 5 分钟，到点重新从首选入口试起，避免网络恢复后一直被降级在备用站上。
static GOOD_HOST: Mutex<Option<(usize, Instant)>> = Mutex::new(None);
const GOOD_HOST_TTL: Duration = Duration::from_secs(300);

/// 复用同一个 HTTP Agent。
///
/// ureq 顶层的 `ureq::get()` 每次都会新建一个 Agent，连接池不跨调用保留 ——
/// 等于每轮刷新都重新 DNS + TCP + TLS 握手一次（行情是几秒一次的高频请求）。
///
/// **TLS 必须显式装配**：ureq 2.x 里 `native-tls` 只是一个“适配器” feature，
/// 它**不会**成为 Agent 的默认 TLS 后端（源码原话 “it is never picked up as a
/// default”）。默认只认 `tls`（rustls）那个 feature，而我们故意没开它。
/// 不装的话所有 https 请求都会直接失败：
/// `Unknown Scheme: cannot make HTTPS request because no TLS backend is configured`
/// —— 之前行情能苟活，全靠第三个 http 入口兜底；东财把 http 也断掉后就整体不可用了。
static AGENT: OnceLock<ureq::Agent> = OnceLock::new();

fn http_agent() -> &'static ureq::Agent {
    AGENT.get_or_init(|| {
        let mut builder = ureq::AgentBuilder::new().timeout(Duration::from_secs(6));
        match ureq::native_tls::TlsConnector::new() {
            Ok(c) => builder = builder.tls_connector(std::sync::Arc::new(c)),
            Err(e) => crate::log_msg(&format!("行情 TLS 初始化失败，https 入口将不可用：{e}")),
        }
        builder.build()
    })
}

/// 本次请求的入口尝试顺序：上次成功的排最前，其余按原顺序绕回来
fn host_order() -> Vec<usize> {
    let pref = GOOD_HOST
        .lock()
        .ok()
        .and_then(|g| *g)
        .filter(|(_, t)| t.elapsed() < GOOD_HOST_TTL)
        .map(|(i, _)| i)
        .unwrap_or(0);
    (0..QUOTE_HOSTS.len())
        .map(|step| (pref + step) % QUOTE_HOSTS.len())
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub secid: String,
    pub code: String,
    pub name: String,
    /// 最新价
    pub price: Option<f64>,
    /// 涨跌额
    pub change: Option<f64>,
    /// 涨跌幅（%）
    pub change_pct: Option<f64>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub prev_close: Option<f64>,
}

fn num(v: &serde_json::Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    // 停牌 / 未开盘时接口可能返回 "-" 之类的字符串
    v.as_str().and_then(|s| s.trim().parse::<f64>().ok())
}

/// 同 `num`，但入参已经是字符串（腾讯返回体是按 `~` 切出来的）
fn num_str(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s == "-" {
        return None;
    }
    s.parse::<f64>().ok()
}

/// 港股代码统一补成 5 位（0700 → 00700）
fn hk_code(s: &str) -> Result<String, String> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.is_empty() || d.len() > 5 {
        return Err(format!("港股代码不正确：{s}"));
    }
    Ok(format!("{d:0>5}"))
}

/// 把用户输入的代码翻译成行情接口的 secid。
/// 支持：600000 / sh600000 / 600000.SH / 000001 / 00700 / hk00700 / AAPL / us.AAPL
pub fn parse_secid(input: &str) -> Result<(String, String), String> {
    let s = input.trim().to_uppercase().replace([' ', '\t'], "");
    if s.is_empty() {
        return Err("请输入股票代码".into());
    }

    // 带点：600000.SH / 00700.HK / US.AAPL
    if let Some((a, b)) = s.split_once('.') {
        let (num, suf) = if a.chars().all(|c| c.is_ascii_digit()) {
            (a, b)
        } else {
            (b, a)
        };
        return match suf {
            "SH" | "SS" => Ok((format!("1.{num}"), num.to_string())),
            "SZ" | "BJ" => Ok((format!("0.{num}"), num.to_string())),
            "HK" => {
                let code = hk_code(num)?;
                Ok((format!("116.{code}"), code))
            }
            "US" => Ok((format!("105.{num}"), num.to_string())),
            _ => Err(format!("无法识别「{input}」，示例：600000、000001、00700、AAPL")),
        };
    }

    // 前缀：sh600000 / sz000001 / bj430047
    for (prefix, market) in [("SH", 1), ("SS", 1), ("SZ", 0), ("BJ", 0)] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if rest.len() == 6 && rest.chars().all(|c| c.is_ascii_digit()) {
                return Ok((format!("{market}.{rest}"), rest.to_string()));
            }
        }
    }
    if let Some(rest) = s.strip_prefix("HK") {
        let code = hk_code(rest)?;
        return Ok((format!("116.{code}"), code));
    }
    if let Some(rest) = s.strip_prefix("US") {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Ok((format!("105.{rest}"), rest.to_string()));
        }
    }

    // 纯数字
    if s.chars().all(|c| c.is_ascii_digit()) {
        return match s.len() {
            6 => {
                // 5/6/9 开头为沪市（含 B 股、基金），其余为深市 / 北交所
                let market = match s.as_bytes()[0] {
                    b'5' | b'6' | b'9' => 1,
                    _ => 0,
                };
                Ok((format!("{market}.{s}"), s.clone()))
            }
            1..=5 => {
                let code = format!("{s:0>5}");
                Ok((format!("116.{code}"), code))
            }
            _ => Err(format!("代码长度不正确：{input}")),
        };
    }

    // 字母 → 美股
    if s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok((format!("105.{s}"), s.clone()));
    }

    Err(format!("无法识别「{input}」，示例：600000、000001、00700、AAPL"))
}

/// 接口一条 data → Quote。
///
/// `stock/get` 不返回市场号（f13），所以 secid 直接用请求时的那一个 ——
/// 这也保证前端 `q.secid` 一定能和它发出去的 secid 对上（港股会被补成 5 位，
/// 例如请求 `116.01810` 接口回的是 `f57 = "01810"`，重新拼反而不一致）。
fn parse_quote(secid: &str, item: &serde_json::Value) -> Option<Quote> {
    let code = item["f57"].as_str()?.trim().to_string();
    if code.is_empty() {
        return None;
    }
    Some(Quote {
        secid: secid.to_string(),
        code,
        name: item["f58"].as_str().unwrap_or("").trim().to_string(),
        price: num(&item["f43"]),
        change: num(&item["f169"]),
        change_pct: num(&item["f170"]),
        open: num(&item["f46"]),
        high: num(&item["f44"]),
        low: num(&item["f45"]),
        prev_close: num(&item["f60"]),
    })
}

/// ureq 的报错又长又带文档链接，只留第一行关键信息
fn short_err(e: &ureq::Error) -> String {
    let s = e.to_string();
    let line = s.lines().next().unwrap_or("").trim().to_string();
    let line = line
        .strip_prefix("Network Error: ")
        .unwrap_or(line.as_str())
        .trim()
        .to_string();
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(120).collect::<String>())
    } else {
        line
    }
}

/// 腾讯只回 GBK；股票名称是中文，直接用 UTF-8 解会变成乱码。
fn decode_gbk(bytes: &[u8]) -> String {
    let (s, _, _) = encoding_rs::GBK.decode(bytes);
    s.into_owned()
}

/// secid（沿用东财风格的「市场号.代码」，库里存的就是这个）→ 腾讯代码。
/// 1.600795 → sh600795   0.300750 → sz300750
/// 116.01810 → hk01810   105.AAPL → usAAPL
fn tx_symbol(secid: &str) -> Option<String> {
    let (market, code) = secid.split_once('.')?;
    Some(match market {
        "1" => format!("sh{code}"),
        "0" => format!("sz{code}"),
        "116" => format!("hk{code}"),
        "105" => format!("us{code}"),
        _ => return None,
    })
}

/// 解析腾讯响应。一行一只：`v_sh600795="1~国电电力~600795~5.15~…";`
/// `wanted` 把腾讯代码映射回调用方给的 secid，这样前端拿到的 secid 一定和它
/// 请求时发出去的一致（港股 / 美股在腾讯那边代码写法与 secid 并不相同）。
fn tx_parse(body: &str, wanted: &HashMap<String, String>) -> Vec<Quote> {
    let mut out = Vec::new();
    for line in body.split(';') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let Some(sym) = key.trim().strip_prefix("v_") else {
            continue;
        };
        let Some(secid) = wanted.get(sym) else {
            continue;
        };
        let f: Vec<&str> = val.trim().trim_matches('"').split('~').collect();
        if f.len() < TX_MIN_FIELDS {
            continue;
        }
        let code = f[TX_CODE].trim();
        if code.is_empty() {
            continue;
        }
        out.push(Quote {
            secid: secid.clone(),
            code: code.to_string(),
            name: f[TX_NAME].trim().to_string(),
            price: num_str(f[TX_PRICE]),
            change: num_str(f[TX_CHANGE]),
            change_pct: num_str(f[TX_CHANGE_PCT]),
            open: num_str(f[TX_OPEN]),
            high: num_str(f[TX_HIGH]),
            low: num_str(f[TX_LOW]),
            prev_close: num_str(f[TX_PREV_CLOSE]),
        });
    }
    out
}

/// 腾讯批量抓取（主源）。一轮刷新只发一次请求。
fn fetch_tx(secids: &[String]) -> Result<Vec<Quote>, String> {
    let mut out: Vec<Quote> = Vec::with_capacity(secids.len());
    for chunk in secids.chunks(TX_BATCH) {
        let mut wanted: HashMap<String, String> = HashMap::new();
        for sid in chunk {
            if let Some(sym) = tx_symbol(sid) {
                wanted.insert(sym, sid.clone());
            }
        }
        if wanted.is_empty() {
            continue;
        }
        // HashMap 顺序不定，排一下让 URL 稳定，方便对着日志复现
        let mut list: Vec<&str> = wanted.keys().map(|s| s.as_str()).collect();
        list.sort_unstable();
        let url = format!("{TX_HOST}/q={}", list.join(","));
        let resp = http_agent()
            .get(&url)
            .set("User-Agent", UA)
            .set("Referer", "https://gu.qq.com/")
            .call()
            .map_err(|e| format!("行情请求失败：{}", short_err(&e)))?;
        let mut bytes: Vec<u8> = Vec::new();
        resp.into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("行情响应读取失败：{e}"))?;
        out.extend(tx_parse(&decode_gbk(&bytes), &wanted));
    }
    Ok(out)
}

/// 单只并发上限。自选股通常十几只，8 路并发下总耗时 ≈ 一次往返；
/// 再高收益不大，反而更容易被行情服务端当成异常流量。
const MAX_PARALLEL: usize = 8;

/// 抓单只行情，入口按“上次成功的优先”逐个尝试。
///
/// 返回 `Ok(None)` = 接口明确说这只代码没有行情（已退市 / 代码写错），
/// 与 `Err`（网络 / 解析失败）分开 —— 前者不该让整块面板报错。
fn fetch_one(secid: &str) -> Result<Option<Quote>, String> {
    let mut last_err = String::from("未知错误");
    for idx in host_order() {
        let url = format!(
            "{}{QUOTE_PATH}?fltt=2&invt=2&fields={QUOTE_FIELDS}&secid={secid}&ut={QUOTE_UT}",
            QUOTE_HOSTS[idx]
        );
        let resp = match http_agent()
            .get(&url)
            .set("User-Agent", UA)
            .set("Referer", "https://quote.eastmoney.com/")
            .set("Accept", "application/json, text/plain, */*")
            .call()
        {
            Ok(r) => r,
            Err(e) => {
                last_err = short_err(&e);
                continue;
            }
        };
        // 响应读取 / JSON 解析失败也要继续试其它入口（截断的响应很常见），
        // 直接 `?` 返回会让剩下的入口根本没机会。
        let text = match resp.into_string() {
            Ok(t) => t,
            Err(e) => {
                last_err = format!("行情响应读取失败：{e}");
                continue;
            }
        };
        let v: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                last_err = format!("行情解析失败：{e}");
                continue;
            }
        };
        let rc = v["rc"].as_i64().unwrap_or(0);
        if rc != 0 {
            // 应用层错误码（例如 ut 失效）对所有代码都一样，换入口也没用
            return Err(format!("行情接口返回错误码 {rc}"));
        }
        // data 为 null 表示该代码没有行情，不是网络问题
        if v["data"].is_null() {
            return Ok(None);
        }
        // 只有真拿到数据才记住这个入口，避免把“连得上但没数据”的入口记成好入口
        if let Ok(mut g) = GOOD_HOST.lock() {
            *g = Some((idx, Instant::now()));
        }
        return Ok(parse_quote(secid, &v["data"]));
    }
    Err(format!("行情请求失败：{last_err}"))
}

/// 备源：东财逐只并发请求，按传入顺序聚合结果。
///
/// 只有“一只都没拿到”才算失败。个别代码查不到（下市 / 写错）会被静默跳过，
/// 网络抖动导致个别失败也不会把整块面板打成错误态 —— 下一轮刷新就补回来了。
fn fetch_em_parallel(secids: &[String]) -> Result<Vec<Quote>, String> {
    if secids.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<Quote> = Vec::with_capacity(secids.len());
    let mut first_err: Option<String> = None;
    for chunk in secids.chunks(MAX_PARALLEL) {
        // thread::scope：直接借用 secids，不需要 Arc；子线程全部结束后才返回
        let joined = std::thread::scope(|s| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|sid| s.spawn(move || fetch_one(sid)))
                .collect();
            handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
        });
        for r in joined {
            match r {
                Ok(Ok(Some(q))) => out.push(q),
                // 代码无效：静默跳过（与原批量接口行为一致）
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(_) => {
                    if first_err.is_none() {
                        first_err = Some("行情线程异常退出".to_string());
                    }
                }
            }
        }
    }
    if out.is_empty() {
        if let Some(e) = first_err {
            return Err(e);
        }
    }
    Ok(out)
}

/// 批量抓取行情。
///
/// 主源是腾讯（一轮刷新只发一个请求），只有拿不到任何数据时才降级到东财逐只
/// 请求。两个源都不通才返回 `Err`。
pub fn fetch_quotes(secids: &[String]) -> Result<Vec<Quote>, String> {
    if secids.is_empty() {
        return Ok(Vec::new());
    }
    match fetch_tx(secids) {
        Ok(v) if !v.is_empty() => Ok(v),
        // 主源通了却一只都没解析出来：多半是腾讯不认的代码（例如北交所），
        // 换东财再试一次，别把用户的自选股直接弄丢
        Ok(_) => fetch_em_parallel(secids),
        Err(e) => {
            crate::log_msg(&format!("腾讯行情不可用，降级到东财：{e}"));
            fetch_em_parallel(secids)
        }
    }
}

/// 查单只股票的名称（添加自选时用来校验代码并补名称）。
/// `Ok(None)` = 代码无效；`Err` = 网络问题，二者含义不同，调用方要分开处理。
pub fn fetch_name(secid: &str) -> Result<Option<String>, String> {
    let list = fetch_quotes(&[secid.to_string()])?;
    Ok(list
        .into_iter()
        .next()
        .map(|q| q.name)
        .filter(|n| !n.is_empty()))
}

// ---------- 开关 ----------

pub fn is_enabled(db: &Db) -> bool {
    db.get_setting(ENABLED_KEY)
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// 显示 / 隐藏股票面板
pub fn set_enabled(app: &AppHandle, db: &Db, enabled: bool) -> Result<(), String> {
    db.set_setting(ENABLED_KEY, if enabled { "true" } else { "false" })?;
    if enabled {
        let win = open_stock_window(app, db)?;
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        persist_cfg(&win, db);
    } else if let Some(win) = app.get_webview_window(STOCK_LABEL) {
        crate::boxwin::hide_win(&win);
        persist_cfg(&win, db);
        // 组件"关闭"只是隐藏窗口，前端定时器不会停 —— 通知它停掉行情轮询
        let _ = win.emit(PAUSE_EVENT, false);
    }
    if enabled {
        // 重新显示时通知前端恢复轮询并立刻刷一次
        let _ = app.emit_to(STOCK_LABEL, PAUSE_EVENT, true);
    }
    let _ = app.emit(ENABLED_EVENT, enabled);
    Ok(())
}

// ---------- 窗口 ----------

/// 位置/大小节流持久化（拖动/缩放时高频触发）
fn persist_cfg(win: &WebviewWindow, db: &Db) {
    if let Ok(mut last) = LAST_SAVE.lock() {
        let now = Instant::now();
        if let Some(t) = *last {
            if now.duration_since(t) < Duration::from_millis(400) {
                return;
            }
        }
        *last = Some(now);
    }
    force_persist_cfg(win, db);
}

fn force_persist_cfg(win: &WebviewWindow, db: &Db) {
    let Ok(pos) = win.outer_position() else { return };
    let Ok(size) = win.outer_size() else { return };
    let mut cfg = db
        .get_stock_window_cfg()
        .ok()
        .flatten()
        .unwrap_or_else(StockWindowCfg::new);
    cfg.x = pos.x;
    cfg.y = pos.y;
    cfg.w = size.width as i32;
    cfg.h = size.height as i32;
    cfg.visible = win.is_visible().unwrap_or(true);
    let _ = db.set_stock_window_cfg(&cfg);
}

/// 创建（或复用）股票面板窗口
pub fn open_stock_window(app: &AppHandle, db: &Db) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(STOCK_LABEL) {
        return Ok(w);
    }
    let cfg = db
        .get_stock_window_cfg()
        .ok()
        .flatten()
        .unwrap_or_else(StockWindowCfg::new);
    let url = WebviewUrl::App("index.html?stocks=1".into());
    let win = WebviewWindowBuilder::new(app, STOCK_LABEL, url)
        .title("股票")
        .inner_size(cfg.w.max(240) as f64, cfg.h.max(160) as f64)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .skip_taskbar(true)
        // 先隐藏建窗，位置校正到物理像素后再显示，避免在高 DPI 上闪一下
        .visible(false)
        .build()
        .map_err(|e| format!("创建股票面板失败: {e}"))?;

    // 配置里存的是 outer_position / outer_size 的返回值（物理像素），
    // 而 builder 的 position / inner_size 用的是逻辑像素，高 DPI 下两者不等，这里校正一次。
    let _ = win.set_position(PhysicalPosition::new(cfg.x, cfg.y));
    let _ = win.set_size(PhysicalSize::new(
        cfg.w.max(240) as u32,
        cfg.h.max(160) as u32,
    ));
    if cfg.visible {
        let _ = win.show();
    }

    boxwin::force_no_taskbar(&win);
    crate::apply_ui_zoom(&win);
    // 原生边缘拉伸的吸附
    crate::resize_snap::install(&win, STOCK_LABEL);

    let app2 = app.clone();
    let win2 = win.clone();
    let db2 = db.clone();
    win.on_window_event(move |event| match event {
        WindowEvent::Moved(_) => persist_cfg(&win2, &db2),
        WindowEvent::Resized(_) => persist_cfg(&win2, &db2),
        WindowEvent::CloseRequested { api, .. } => {
            // 关闭 = 隐藏并取消组件勾选（与设置页状态保持一致）
            api.prevent_close();
            crate::boxwin::hide_win(&win2);
            force_persist_cfg(&win2, &db2);
            let _ = db2.set_setting(ENABLED_KEY, "false");
            let _ = app2.emit(ENABLED_EVENT, false);
        }
        _ => {}
    });

    Ok(win)
}

/// 启动时按开关恢复股票面板
pub fn restore_on_startup(app: &AppHandle, db: &Db) {
    if is_enabled(db) {
        if let Err(e) = open_stock_window(app, db) {
            crate::log_msg(&format!("open_stock_window failed: {e}"));
        }
    }
}

// ---------- 拖动吸附 ----------

/// 面板拖动后的松手吸附（与分区/便签窗口同一套 find_snap 逻辑）
pub fn start_stock_drag(app: AppHandle, force_moved: bool) -> Result<(), String> {
    let win = app
        .get_webview_window(STOCK_LABEL)
        .ok_or_else(|| "股票面板不存在".to_string())?;
    if DRAGGING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    std::thread::spawn(move || {
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                DRAGGING.store(false, Ordering::SeqCst);
            }
        }
        let _g = Guard;

        let mut last: Option<(i32, i32)> = None;
        let mut stable = 0u32;
        let mut ever_moved = force_moved;
        let begin = Instant::now();
        let mut ticks = 0u32;
        loop {
            ticks += 1;
            if ticks > 240 {
                break;
            }
            let Ok(pos) = win.outer_position() else { break };
            let Ok(size) = win.outer_size() else { break };
            if let Some(lp) = last {
                if lp == (pos.x, pos.y) {
                    stable += 1;
                } else {
                    ever_moved = true;
                    stable = 0;
                }
            }
            last = Some((pos.x, pos.y));
            if !ever_moved && begin.elapsed() > Duration::from_millis(1500) {
                break;
            }
            if stable >= 10 {
                if !ever_moved {
                    break;
                }
                // 设置里关了吸附 → 不做松手对齐，直接结束
                if !boxwin::snap_enabled(&app) {
                    break;
                }
                let mut others: Vec<(i64, i32, i32, i32, i32)> = Vec::new();
                if let Some(bw) = app.try_state::<boxwin::BoxWindows>() {
                    for id in bw.ids() {
                        if let Some(l) = bw.label_for(id) {
                            if let Some(w) = app.get_webview_window(&l) {
                                if let (Ok(op), Ok(os)) = (w.outer_position(), w.outer_size()) {
                                    others.push((id, op.x, op.y, os.width as i32, os.height as i32));
                                }
                            }
                        }
                    }
                }
                let threshold = app
                    .state::<Db>()
                    .get_setting("snapThreshold")
                    .ok()
                    .flatten()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(20);
                if let Some((_, nx, ny)) = boxwin::find_snap(
                    pos.x,
                    pos.y,
                    size.width as i32,
                    size.height as i32,
                    &others,
                    threshold,
                ) {
                    let _ = win.set_position(PhysicalPosition::new(nx, ny));
                    // 吸附后的最终位置要立刻落盘：Moved 事件有 400ms 节流，
                    // 而吸附正好发生在刚拖完的高发窗口里，不补这一下重启后位置会回退。
                    if let Some(db) = app.try_state::<crate::db::Db>() {
                        force_persist_cfg(&win, &db);
                    }
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把 `stock/get` 的字段编号钉死。改错编号不会报错，只会让价格静默变成 None。
    #[test]
    fn parse_quote_maps_stock_get_fields() {
        let data = serde_json::json!({
            "f57": "600795", "f58": "国电电力",
            "f43": 5.15, "f44": 5.19, "f45": 5.12, "f46": 5.18, "f60": 5.19,
            "f169": -0.04, "f170": -0.77
        });
        let q = parse_quote("1.600795", &data).expect("应能解析");
        assert_eq!(q.secid, "1.600795");
        assert_eq!(q.code, "600795");
        assert_eq!(q.name, "国电电力");
        assert_eq!(q.price, Some(5.15));
        assert_eq!(q.high, Some(5.19));
        assert_eq!(q.low, Some(5.12));
        assert_eq!(q.open, Some(5.18));
        assert_eq!(q.prev_close, Some(5.19));
        assert_eq!(q.change, Some(-0.04));
        assert_eq!(q.change_pct, Some(-0.77));
    }

    /// 港股 secid 必须原样回传：接口回的是 5 位 f57（"01810"），
    /// 重新按 code 拼会和前端发出来的 secid 对不上，行情就整片空白。
    #[test]
    fn parse_quote_keeps_requested_secid() {
        let data = serde_json::json!({ "f57": "01810", "f58": "小米集团-W", "f43": 26.62 });
        let q = parse_quote("116.01810", &data).expect("应能解析");
        assert_eq!(q.secid, "116.01810");
        assert_eq!(q.code, "01810");
    }

    /// 停牌 / 未开盘时接口回 "-"，不能让整条记录被丢掉。
    #[test]
    fn parse_quote_tolerates_dash() {
        let data = serde_json::json!({ "f57": "600001", "f58": "邯郸钢铁", "f43": "-", "f170": "-" });
        let q = parse_quote("1.600001", &data).expect("应能解析");
        assert_eq!(q.price, None);
        assert_eq!(q.change_pct, None);
        assert_eq!(q.name, "邯郸钢铁");
    }

    /// secid → 腾讯代码。库里存的是东财风格的 secid，转换错了会静默少一行行情。
    #[test]
    fn tx_symbol_maps_all_markets() {
        assert_eq!(tx_symbol("1.600795").as_deref(), Some("sh600795"));
        assert_eq!(tx_symbol("0.300750").as_deref(), Some("sz300750"));
        assert_eq!(tx_symbol("116.01810").as_deref(), Some("hk01810"));
        assert_eq!(tx_symbol("105.AAPL").as_deref(), Some("usAAPL"));
        assert_eq!(tx_symbol("888.XXX"), None);
        assert_eq!(tx_symbol("nonsense"), None);
    }

    /// 腾讯返回体是 GBK，名称必须解成中文而不是乱码。
    #[test]
    fn decode_gbk_reads_chinese_name() {
        let bytes: &[u8] = &[0xB9, 0xFA, 0xB5, 0xE7, 0xB5, 0xE7, 0xC1, 0xA6];
        assert_eq!(decode_gbk(bytes), "国电电力");
    }

    /// 腾讯响应解析：字段索引、GBK 名称、secid 原样回传。
    #[test]
    fn tx_parse_reads_quotes() {
        let mut f = vec![""; 36];
        f[0] = "1";
        f[TX_NAME] = "国电电力";
        f[TX_CODE] = "600795";
        f[TX_PRICE] = "5.15";
        f[TX_PREV_CLOSE] = "5.19";
        f[TX_OPEN] = "5.18";
        f[TX_CHANGE] = "-0.04";
        f[TX_CHANGE_PCT] = "-0.77";
        f[TX_HIGH] = "5.19";
        f[TX_LOW] = "5.12";
        let body = format!("v_sh600795=\"{}\";", f.join("~"));

        let mut wanted = HashMap::new();
        wanted.insert("sh600795".to_string(), "1.600795".to_string());
        let list = tx_parse(&body, &wanted);
        assert_eq!(list.len(), 1, "应当解析出一只：{body}");
        let q = &list[0];
        assert_eq!(q.secid, "1.600795");
        assert_eq!(q.code, "600795");
        assert_eq!(q.name, "国电电力");
        assert_eq!(q.price, Some(5.15));
        assert_eq!(q.prev_close, Some(5.19));
        assert_eq!(q.open, Some(5.18));
        assert_eq!(q.change, Some(-0.04));
        assert_eq!(q.change_pct, Some(-0.77));
        assert_eq!(q.high, Some(5.19));
        assert_eq!(q.low, Some(5.12));
    }

    /// 没请求过的代码（腾讯多回、或代码拼错）不能凭空造一条出来。
    #[test]
    fn tx_parse_skips_unknown_symbols() {
        let mut f = vec![""; 36];
        f[TX_NAME] = "幽灵";
        f[TX_CODE] = "999999";
        let body = format!("v_sh999999=\"{}\";", f.join("~"));
        let mut wanted = HashMap::new();
        wanted.insert("sh600795".to_string(), "1.600795".to_string());
        assert!(tx_parse(&body, &wanted).is_empty());
    }

    /// 真实网络回归。行情这块踩过两次坑且都很难查：
    /// ① 东财下线 `ulist.np/get`；② ureq 的 native-tls 没显式装配 → https 全挂。
    /// 平时不跑（依赖外网），手动执行：
    /// `cargo test --lib stock::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_fetch_quotes_works() {
        let secids: Vec<String> = ["1.600795", "0.300750", "1.601988", "116.01810"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let list = fetch_quotes(&secids).expect("行情请求应当成功");
        assert_eq!(list.len(), secids.len(), "每只代码都该拿到行情：{list:?}");
        for q in &list {
            assert!(!q.name.is_empty(), "缺少名称：{q:?}");
            assert!(q.price.is_some(), "缺少最新价：{q:?}");
        }
        assert!(
            list.iter().any(|q| q.secid == "116.01810"),
            "港股 secid 应原样返回：{list:?}"
        );
    }
}
