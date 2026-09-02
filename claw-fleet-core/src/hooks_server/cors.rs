//! 跨源访问:只给**手机直连**需要的那两条路,而且只在有 token 门时才开。
//!
//! 为什么需要:手机上那个页面的 origin 是中转的域名(或原生壳的假域名),它去问
//! 一台 `fleet serve` 主机就是跨源请求。没有 `Access-Control-Allow-Origin`,浏览器
//! 会在页面读到响应之前把它拦掉 —— 服务端明明答了 200,前端只看到一个网络错误。
//!
//! 为什么只开两条路:手机的数据面只有 `POST /mobile_rpc` 与 `GET /events`。其余
//! 几百个路由(proc exec、settings、凭证、文件浏览)没有任何跨源使用者,给它们开
//! CORS 只是白扩暴露面。
//!
//! 为什么以 token 门为条件:`fleet webui` 那个端口**本身没有认证**(它自己的启动
//! 日志就写着「这个端口能起 agent 会话,必须自己在前面放网关」)。在那种端口上无
//! 条件回 `Allow-Origin: *`,等于让用户浏览器里**任何一个网页**都能驱动他的 Fleet。
//! 所以:认证开着(有 admin/scoped token 要验)才发 CORS 头;认证关掉的部署要跨源
//! 就得在它自己的网关上配 —— 那本来也是那种部署的既定分工。
//!
//! 用 `*` 而不是回写请求的 Origin,是因为这里**不用 cookie**:凭证走
//! `Authorization: Bearer`(或 SSE 的 `?token=`),而带 `*` 的响应浏览器不会附带
//! cookie。所以 `*` 在这里不构成「凭浏览器身份被冒用」那类风险,而回写 Origin 反倒
//! 要多维护一张名单。

use tiny_http::Header;

/// 允许跨源的路径 —— 手机数据面的全部。
const CORS_PATHS: [&str; 2] = [crate::routes::MOBILE_RPC, "/events"];

pub fn is_cors_path(path: &str) -> bool {
    CORS_PATHS.contains(&path)
}

/// 这个部署是否对外开放跨源。`auth_disabled` 为真(前面有自己的网关)时不开 ——
/// 见模块头的理由。
pub fn cors_enabled(auth_disabled: bool) -> bool {
    !auth_disabled
}

/// 该加到响应上的 CORS 头。不开放时是空的 —— 调用方照常 `with_header` 遍历,
/// 不必分叉。
pub fn headers(auth_disabled: bool, path: &str) -> Vec<Header> {
    if !cors_enabled(auth_disabled) || !is_cors_path(path) {
        return Vec::new();
    }
    header_set()
}

/// 预检要回的那一组头。`Authorization` 与 `Content-Type` 必须在 allow 列表里:
/// 前者是 token,后者是 `application/json`(它让 POST 变成「非简单请求」,于是浏览器
/// 才会先发 OPTIONS)。
fn header_set() -> Vec<Header> {
    [
        "Access-Control-Allow-Origin: *",
        "Access-Control-Allow-Methods: GET, POST, OPTIONS",
        "Access-Control-Allow-Headers: authorization, content-type",
        // 预检结果缓存 10 分钟:手机每次请求前都多一个往返,在移动链路上是实打实
        // 的延迟。
        "Access-Control-Max-Age: 600",
    ]
    .iter()
    .map(|h| h.parse::<Header>().expect("static CORS header parses"))
    .collect()
}

/// 这是不是一次该由我们直接回掉的预检。
///
/// **预检必须在认证之前答**:浏览器发 OPTIONS 时**不带** `Authorization` 头(那正是
/// 它要问「带这个头行不行」的东西)。放到认证之后,预检会拿到 401,于是真正的请求
/// 永远发不出去 —— 而症状只是「跨源请求失败」,看不出是预检死在门口。
pub fn is_preflight(method: &tiny_http::Method, path: &str, auth_disabled: bool) -> bool {
    cors_enabled(auth_disabled) && method == &tiny_http::Method::Options && is_cors_path(path)
}

/// 回一个 204 预检响应。
pub fn preflight_response() -> tiny_http::Response<std::io::Empty> {
    let mut res = tiny_http::Response::empty(204);
    for h in header_set() {
        res.add_header(h);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_the_two_mobile_paths() {
        assert!(is_cors_path("/mobile_rpc"));
        assert!(is_cors_path("/events"));
        // 其余路由没有跨源使用者 —— 给它们开只是白扩暴露面。
        assert!(!is_cors_path("/settings"));
        assert!(!is_cors_path("/proc/exec"));
        assert!(!is_cors_path("/v1/sessions"));
        assert!(!is_cors_path("/"));
    }

    /// 无认证的端口不发 CORS 头:否则任何网页都能驱动这台 Fleet。
    #[test]
    fn stays_shut_when_auth_is_disabled() {
        assert!(!cors_enabled(true));
        assert!(headers(true, "/mobile_rpc").is_empty());
        assert!(!is_preflight(&tiny_http::Method::Options, "/mobile_rpc", true));
    }

    #[test]
    fn opens_when_a_token_gate_is_active() {
        assert!(cors_enabled(false));
        let hs = headers(false, "/mobile_rpc");
        assert!(!hs.is_empty());
        let rendered: Vec<String> = hs
            .iter()
            .map(|h| format!("{}: {}", h.field.as_str().as_str(), h.value.as_str()))
            .collect();
        assert!(rendered.iter().any(|h| h == "Access-Control-Allow-Origin: *"));
        // token 与 JSON 的 content-type 必须在 allow 列表里,否则预检就把请求
        // 判死在门口。
        assert!(rendered
            .iter()
            .any(|h| h.to_ascii_lowercase().contains("authorization")
                && h.to_ascii_lowercase().contains("content-type")));
    }

    #[test]
    fn non_cors_paths_get_no_headers_even_with_auth_on() {
        assert!(headers(false, "/settings").is_empty());
    }

    #[test]
    fn preflight_is_only_options_on_a_cors_path() {
        assert!(is_preflight(&tiny_http::Method::Options, "/events", false));
        assert!(!is_preflight(&tiny_http::Method::Post, "/mobile_rpc", false));
        assert!(!is_preflight(&tiny_http::Method::Options, "/settings", false));
    }

    #[test]
    fn preflight_response_carries_the_headers() {
        let res = preflight_response();
        assert_eq!(res.status_code().0, 204);
        assert!(res
            .headers()
            .iter()
            .any(|h| h.field.equiv("access-control-allow-origin")));
    }
}
