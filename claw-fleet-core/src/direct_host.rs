//! 「直连」——把这台机器的 HTTP 数据面交给手机,不经中转。
//!
//! 手机端的设备簿现在认两种设备:经中转配对的,和直连一台 HTTP 主机的
//! (`mobile-web/src/devices.ts` 的 `kind`)。后者手填地址和 token 是能用的,但
//! 那是一串 64 位十六进制 token 在手机键盘上敲一遍 —— 所以这里出一张码,扫一下
//! 就把「地址 + token」一起交过去。
//!
//! ## 为什么地址必须由用户给
//!
//! 这台机器**不知道**自己在手机那边叫什么。它的 serve 端口绑在 127.0.0.1 上,
//! 而手机要访问得穿过局域网、隧道或反代 —— 那个对外地址只有部署它的人知道。所以
//! 地址是一项配置(存在 `~/.fleet/direct-host.json`),而不是探测出来的。
//!
//! ## 为什么必须是 https(除 localhost)
//!
//! 手机上那个页面是 https 发的(中转域名,或原生壳的 `https://fleet.local`),而
//! 浏览器不允许一个 https 页面去 fetch 明文 http —— 混合内容。所以 `http://` 的
//! 局域网地址填进来只会得到一次「连不上」,而原因在界面上看不出来。这里在**出码
//! 之前**就把这个判断做掉,让桌面端能当场说清楚,而不是把一张注定失败的码交给
//! 用户。localhost 是例外:浏览器把它当可信来源(但手机也访问不到它,所以它只在
//! 同机调试时有意义)。
//!
//! ## 码里编的是什么
//!
//! `<移动端页面地址>/#h=<对外地址>&t=<token>` —— 复用手机端那条已有的 fragment
//! 入口(`devices.ts::consumeHashSecret` 的同一处),所以扫码在三种客户端上都能
//! 落地:系统相机打开中转托管的那份 PWA、已装的 PWA、原生壳内扫码。fragment 不
//! 进请求行,所以 token 不会落进任何服务端日志。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_FILE_NAME: &str = "direct-host.json";

/// 直连配置。
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DirectHostConfig {
    /// 手机能访问到的地址(origin,可带路径前缀)。空 = 还没配过。
    #[serde(default)]
    pub base_url: String,
    /// 手填的 token(可选)。空 = 用 `~/.fleet/token`(那是本机 `fleet serve`
    /// 启动时写下的)。
    ///
    /// 留这个口子是因为对外那个端点不一定是本机的 serve:可能是云上那台、也可能
    /// 是一层反代自己那套 token。那种情况下本机的 token 文件根本不是手机要过的
    /// 那道门。
    #[serde(default)]
    pub token: String,
}

fn config_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

pub fn load_config() -> DirectHostConfig {
    let Some(p) = config_path() else {
        return DirectHostConfig::default();
    };
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 存配置并回一份新现状(界面直接用它渲染,不必再问一次)。
///
/// `token` 为空表示「用本机 `~/.fleet/token`」——不是「清空 token」,因为空 token
/// 本来就等于没有。
pub fn set_config(base_url: &str, token: &str) -> Result<DirectHostStatus, String> {
    let cfg = DirectHostConfig {
        base_url: base_url.trim().to_string(),
        token: token.trim().to_string(),
    };
    save_config(&cfg).map_err(|e| format!("save direct host config: {e}"))?;
    Ok(status())
}

/// 存配置(0600 —— 这个文件可能装着一个手填的 token,与 mobile-relay.json 同级
/// 敏感)。
pub fn save_config(cfg: &DirectHostConfig) -> std::io::Result<()> {
    let p = config_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no fleet dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&p, serde_json::to_string_pretty(cfg).unwrap_or_default())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 一个对外地址能不能用,以及不能用的原因。`None` = 可用。
///
/// 纯函数:判断只看字符串,好让它在没有网络、没有 `~/.fleet` 的情况下被单测钉住。
pub fn url_problem(raw: &str) -> Option<UrlProblem> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(UrlProblem::Empty);
    }
    let lower = trimmed.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        return host_problem(r, true);
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return Some(UrlProblem::NoScheme);
    };
    host_problem(rest, false)
}

fn host_problem(rest: &str, https: bool) -> Option<UrlProblem> {
    let host = rest.split(['/', ':', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Some(UrlProblem::NoHost);
    }
    let is_local = host == "localhost" || host == "127.0.0.1" || host == "[::1]";
    if !https && !is_local {
        // 手机上那个页面是 https 的,浏览器会拦掉它对明文 http 的请求。出码之前
        // 就说清楚,别把一张注定失败的码交给用户。
        return Some(UrlProblem::PlainHttp);
    }
    if is_local {
        // 语法上没错,但手机访问不到本机回环地址 —— 只在同机调试时有意义,所以
        // 是一条**提醒**而不是拒绝(调用方据此显示黄字而不是红字)。
        return Some(UrlProblem::Loopback);
    }
    None
}

/// 地址不可用/需要提醒的原因。每一条都对应界面上一句能照着做的话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UrlProblem {
    /// 还没填。
    Empty,
    /// 没写协议(要 `https://…`)。
    NoScheme,
    /// 只有协议没有主机。
    NoHost,
    /// 明文 http 且不是本机 —— 手机上的 https 页面连不了它(混合内容)。
    PlainHttp,
    /// 回环地址:语法没问题,但手机访问不到。仅同机调试有意义。
    Loopback,
}

impl UrlProblem {
    /// 是否严重到不该出码。回环只是提醒(同机调试确实用得上),其余都是拒绝。
    pub fn blocks_qr(self) -> bool {
        !matches!(self, UrlProblem::Loopback)
    }
}

/// 拼出扫码链接。
///
/// `page_base` 是**手机端页面**的地址(中转的 origin —— 那份 PWA 就托管在那里);
/// `host_base` 是这台机器对外的地址;`token` 是 `~/.fleet/token` 里那个 admin
/// token。三者都编进 fragment 之后的部分,所以 token 不进请求行。
pub fn direct_url(page_base: &str, host_base: &str, token: &str) -> String {
    format!(
        "{}/#h={}&t={}",
        page_base.trim_end_matches('/'),
        urlencode(host_base.trim().trim_end_matches('/')),
        urlencode(token.trim()),
    )
}

/// 最小的 percent-encoding:只放过 URL 安全字符。自己写而不是引一个 crate ——
/// 要编的东西只有一个 URL 和一个十六进制 token,标准库足够。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 手机直连要过的那道门的 token。
///
/// 顺序:配置里手填的那个优先,否则 `~/.fleet/token`(本机 `fleet serve` 启动时
/// 写下的)。两处都空 = 这台机器上没有 token 门控的 serve 跑过,也就出不了码 ——
/// 而那不是一个可以糊过去的状态:没有 token 门,跨源本来也不会开(见
/// hooks_server::cors 的理由)。
pub fn effective_token() -> Option<String> {
    let manual = load_config().token.trim().to_string();
    if !manual.is_empty() {
        return Some(manual);
    }
    read_local_token()
}

/// `~/.fleet/token` —— `fleet serve` 启动时写的那个。
pub fn read_local_token() -> Option<String> {
    let p = crate::launchd::token_file_path()?;
    let t = fs::read_to_string(p).ok()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// 给「移动端」面板的一份直连现状。
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DirectHostStatus {
    /// 已配的对外地址(空 = 没配过)。
    pub base_url: String,
    /// 地址的问题(`None` = 没问题)。
    pub problem: Option<UrlProblem>,
    /// 有没有可用的 token(手填的或 `~/.fleet/token`)。没有就出不了码。
    pub token_present: bool,
    /// 这个 token 是手填的还是取自本机 `~/.fleet/token` —— 界面据此说清楚
    /// 「码里用的是哪一个」。
    pub token_manual: bool,
    /// 出得了码吗。
    pub ready: bool,
}

pub fn status() -> DirectHostStatus {
    let cfg = load_config();
    let problem = url_problem(&cfg.base_url);
    let token_manual = !cfg.token.trim().is_empty();
    let token_present = token_manual || read_local_token().is_some();
    let ready = token_present && !problem.map(UrlProblem::blocks_qr).unwrap_or(false);
    DirectHostStatus { base_url: cfg.base_url, problem, token_present, token_manual, ready }
}

/// 手机端页面托管在哪 —— 也就是扫码之后浏览器该打开的地方。
///
/// 取的是**这台机器**的 mobile-relay 配置里那个 relay 地址:那份 PWA 就托管在
/// 中转上(中转同时是静态站点)。没配过中转时 `load_config()` 给的是按区域选的
/// 默认值,所以这里永远有值 —— 而且这条路径**不需要**中转真的可用:直连设备根本
/// 不连中转,那个地址只是页面的来源。
///
/// 由主机自己算而不是让调用方传:远端 workspace 里,答这个问题的应该是那台机器
/// 自己(它知道自己的中转配置),而不是本地桌面端替它猜。
pub fn page_base() -> String {
    crate::mobile_relay::load_config().relay_url
}

/// 扫码链接的文本形式(给复制按钮)。
pub fn direct_url_text_here() -> Result<String, String> {
    direct_url_text(&page_base())
}

/// 扫码链接的二维码(SVG)。
pub fn direct_qr_svg_here() -> Result<String, String> {
    direct_qr_svg(&page_base())
}

fn direct_url_text(page_base: &str) -> Result<String, String> {
    let cfg = load_config();
    if let Some(p) = url_problem(&cfg.base_url) {
        if p.blocks_qr() {
            return Err(format!("direct host url unusable: {p:?}"));
        }
    }
    let token = effective_token()
        // 别在这里教用户去跑 serve:那是无头形态(云容器 / 桌面端替远端主机拉起)
        // 的进程,不是一条用户操作。直连要指向的是**另一台已经部署好的**主机,
        // 所以缺 token 时该说的是「填那台的 admin token」。
        .ok_or_else(|| "no token: enter the admin token of the host you are pointing at".to_string())?;
    Ok(direct_url(page_base, &cfg.base_url, &token))
}

/// 扫码链接的二维码(SVG)。与中转那张码同一套渲染参数,好让两张码在面板上看起来
/// 是一对而不是两个风格。
fn direct_qr_svg(page_base: &str) -> Result<String, String> {
    let url = direct_url_text(page_base)?;
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|e| format!("qr encode: {e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_https_host() {
        assert_eq!(url_problem("https://fleet.example.com"), None);
        assert_eq!(url_problem("https://fleet.example.com:8443/prefix"), None);
        assert_eq!(url_problem("  https://fleet.example.com/  "), None);
    }

    /// 手机上那个页面是 https 的,浏览器会拦掉它对明文 http 的请求。这一条要在
    /// **出码之前**拦住,否则用户扫完只会得到一次「连不上」。
    #[test]
    fn refuses_plain_http_on_a_real_host() {
        assert_eq!(url_problem("http://192.168.1.5:8080"), Some(UrlProblem::PlainHttp));
        assert!(UrlProblem::PlainHttp.blocks_qr());
    }

    /// 回环只是提醒:语法没错,同机调试确实用得上,但手机访问不到。
    #[test]
    fn flags_loopback_without_blocking() {
        assert_eq!(url_problem("http://127.0.0.1:18110"), Some(UrlProblem::Loopback));
        assert_eq!(url_problem("https://localhost:18110"), Some(UrlProblem::Loopback));
        assert!(!UrlProblem::Loopback.blocks_qr());
    }

    #[test]
    fn refuses_junk() {
        assert_eq!(url_problem(""), Some(UrlProblem::Empty));
        assert_eq!(url_problem("   "), Some(UrlProblem::Empty));
        assert_eq!(url_problem("fleet.example.com"), Some(UrlProblem::NoScheme));
        assert_eq!(url_problem("ws://fleet.example.com"), Some(UrlProblem::NoScheme));
        assert_eq!(url_problem("https://"), Some(UrlProblem::NoHost));
    }

    /// 冻结向量:手机侧 `mobile-web/src/devices.test.ts` 的
    /// `parseHostParam` 用同一串字符断言解析结果。两边各钉一次,格式就不可能
    /// 单边漂移 —— 而漂移的症状只是「扫码打开一个什么都不做的页面」。
    #[test]
    fn builds_the_scan_url_with_both_parts_in_the_fragment() {
        let url = direct_url(
            "https://fleet-relay.example.com/",
            "https://fleet.example.com/",
            "abc123",
        );
        assert_eq!(
            url,
            "https://fleet-relay.example.com/#h=https%3A%2F%2Ffleet.example.com&t=abc123"
        );
        // token 与地址都在 `#` 之后 —— fragment 不进请求行,所以 token 不会落进
        // 任何服务端日志。这是这个格式存在的理由之一,值得钉住。
        let (before, after) = url.split_once('#').unwrap();
        assert!(!before.contains("abc123"));
        assert!(after.contains("abc123"));
    }

    #[test]
    fn encodes_characters_that_would_break_the_fragment() {
        let url = direct_url("https://p.example.com", "https://h.example.com", "a&b=c d/e");
        assert!(url.ends_with("&t=a%26b%3Dc%20d%2Fe"), "got {url}");
    }
}
