//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_memories(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let mut memories = Vec::new();
                for source in sources {
                    memories.extend(source.list_memories());
                }
                let body = serde_json::to_string(&memories).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_memory_content(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Try each source for memory content; fall back to direct read for Claude Code
                let result = sources.iter()
                    .find_map(|s| s.get_memory_content(&file_path).ok())
                    .or_else(|| memory::read_memory_file(&file_path).ok());
                match result {
                    Some(content) => {
                        let body = serde_json::to_string(&content).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    None => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

pub(crate) fn route_memory_history(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Aggregate history from all sources; fall back to direct trace
                let mut history = Vec::new();
                for source in sources {
                    let h = source.get_memory_history(&file_path);
                    if !h.is_empty() {
                        history = h;
                        break;
                    }
                }
                if history.is_empty() {
                    history = memory::trace_memory_history(&file_path);
                }
                let body = serde_json::to_string(&history).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_handoff_chain(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw = query.get("session").map(|s| s.as_str()).unwrap_or("");
                let sid = percent_decode_str(raw).decode_utf8_lossy().to_string();
                let chain = crate::handoff::chain_containing(&sid);
                let body = serde_json::to_string(&chain).unwrap_or_default();
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }
