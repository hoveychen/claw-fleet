use super::*;

// ── "Will this URL render inside an iframe?" probe ────────────────────────────
//
// A detail-column tab can hold a web page (an IDE tab that happens to be a
// browser), which is an <iframe> underneath. A site that sends `X-Frame-Options`
// or a CSP `frame-ancestors` directive refuses to render there — and the browser
// gives the embedding page no way to notice: a refused frame still fires `load`
// (for its own error page) and its document is cross-origin, so the webview can
// read neither the status nor the headers. It would just sit there blank.
//
// The headers are perfectly readable from the host, though, so this answers the
// question the webview cannot ask. Local-only by nature, like `reveal_path`: the
// subject is whether *this* desktop's webview can frame the URL, so it has to be
// probed from this machine's network. Asking a remote probe host would answer a
// different question, which is why this is deliberately not on the Backend trait.

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UrlEmbedProbe {
    embeddable: bool,
    /// The refusing header, shown verbatim in the tab so the reason is legible
    /// rather than a bare "can't show this". `None` when embeddable.
    reason: Option<String>,
    /// Status of the probe response, when one arrived.
    status: Option<u16>,
}

/// Verdict from the two headers that govern framing. `Some(reason)` = refused.
///
/// Fails *open*: anything unrecognised counts as embeddable, so an exotic header
/// leaves the page to render rather than hiding it behind an error card we can't
/// justify.
pub(crate) fn embed_verdict(xfo: Option<&str>, csp: Option<&str>) -> Option<String> {
    if let Some(raw) = xfo {
        let v = raw.trim().to_ascii_lowercase();
        // ALLOW-FROM is obsolete and could only ever name an http(s) origin,
        // which this webview (tauri://localhost) is not.
        if v.starts_with("deny") || v.starts_with("sameorigin") || v.starts_with("allow-from") {
            return Some(format!("X-Frame-Options: {}", raw.trim()));
        }
    }
    if let Some(list) = csp.and_then(frame_ancestors) {
        // A site's allow-list never names the webview's own origin, so only a
        // bare wildcard can admit us. `'none'`, `'self'`, or any host list all
        // mean refused.
        if !list.split_whitespace().any(|tok| tok == "*") {
            return Some(format!("Content-Security-Policy: frame-ancestors {list}"));
        }
    }
    None
}

/// The `frame-ancestors` source list out of a CSP header value. `None` when the
/// header carries no such directive (CSP then places no limit on framing) or
/// carries it with an empty list (malformed — fail open).
fn frame_ancestors(csp: &str) -> Option<String> {
    csp.split(';')
        .map(str::trim)
        .find(|d| {
            d.split_whitespace()
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
        })
        .map(|d| d.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
        .filter(|list| !list.is_empty())
}

#[tauri::command(async)]
pub(crate) fn probe_url_embeddable(url: String) -> UrlEmbedProbe {
    // http(s) only: nothing else can be framed anyway, and an arbitrary scheme
    // has no business reaching the http client.
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return UrlEmbedProbe {
            embeddable: false,
            reason: Some(format!("unsupported URL: {url}")),
            status: None,
        };
    }
    // A `command(async)` body runs on a tokio worker, where reqwest::blocking
    // panics and the swallowed panic leaves the invoke pending forever — see
    // claw_fleet_core::off_runtime.
    let probed = claw_fleet_core::off_runtime::off_runtime(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .map_err(|e| e.to_string())?;
        // GET rather than HEAD: plenty of servers answer HEAD with 405, or omit
        // the very headers we came for. The body is never read — dropping the
        // response ends the transfer.
        let resp = client.get(&url).send().map_err(|e| e.to_string())?;
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        Ok::<_, String>((
            resp.status().as_u16(),
            header("x-frame-options"),
            header("content-security-policy"),
        ))
    });
    match probed {
        Ok(Ok((status, xfo, csp))) => {
            let reason = embed_verdict(xfo.as_deref(), csp.as_deref());
            UrlEmbedProbe {
                embeddable: reason.is_none(),
                reason,
                status: Some(status),
            }
        }
        // A probe that never completed (offline, DNS, TLS) says nothing about
        // framing: render the frame and let the page speak for itself.
        _ => UrlEmbedProbe {
            embeddable: true,
            reason: None,
            status: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::embed_verdict;

    #[test]
    fn no_headers_is_embeddable() {
        assert_eq!(embed_verdict(None, None), None);
    }

    #[test]
    fn xfo_deny_and_sameorigin_refuse() {
        assert!(embed_verdict(Some("DENY"), None).is_some());
        assert!(embed_verdict(Some("sameorigin"), None).is_some());
        assert!(embed_verdict(Some(" ALLOW-FROM https://example.com"), None).is_some());
    }

    #[test]
    fn unknown_xfo_value_fails_open() {
        assert_eq!(embed_verdict(Some("allowall"), None), None);
    }

    #[test]
    fn frame_ancestors_refuses_unless_wildcard() {
        assert!(embed_verdict(None, Some("frame-ancestors 'none'")).is_some());
        assert!(embed_verdict(None, Some("default-src 'self'; frame-ancestors 'self'")).is_some());
        assert_eq!(embed_verdict(None, Some("frame-ancestors *")), None);
    }

    #[test]
    fn csp_without_frame_ancestors_does_not_restrict_framing() {
        assert_eq!(embed_verdict(None, Some("default-src 'self'; img-src *")), None);
    }

    #[test]
    fn malformed_empty_frame_ancestors_fails_open() {
        assert_eq!(embed_verdict(None, Some("frame-ancestors")), None);
    }

    #[test]
    fn the_reason_names_the_refusing_header() {
        let r = embed_verdict(Some("DENY"), None).unwrap();
        assert!(r.contains("X-Frame-Options"), "{r}");
        let r = embed_verdict(None, Some("frame-ancestors 'self'")).unwrap();
        assert!(r.contains("frame-ancestors"), "{r}");
    }
}
