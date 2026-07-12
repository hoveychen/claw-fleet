//! fleet-relay — generic WebSocket pairing relay with Web Push.
//!
//! Pairs an "agent" (e.g. the Fleet desktop app, dialing out from behind NAT)
//! with "clients" (e.g. a mobile web app) that present the same secret, and
//! forwards opaque JSON frames between the two roles. The relay carries no
//! Fleet business semantics — any project can reuse it.
//!
//! Deploy notes (muvee):
//!   * container listens on :8080 (Traefik terminates TLS and forwards here)
//!   * pin RELAY_VAPID_KEY so browser push subscriptions survive redeploys
//!   * mount/point RELAY_DATA_DIR at a persistent path (push subscriptions)
//!   * RELAY_STATIC_DIR holds the mobile web bundle (served at /)

mod frames;
mod harmony_push;
mod push;
mod registry;
mod ws;
#[cfg(test)]
mod ws_e2e_tests;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use tower_http::services::{ServeDir, ServeFile};

use harmony_push::HarmonyPush;
use push::Push;
use registry::Registry;

pub struct AppState {
    pub registry: Registry,
    pub push: Push,
    /// HarmonyOS Push Kit channel; `None` unless RELAY_HARMONY_* creds are set.
    pub harmony: Option<HarmonyPush>,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// The API part of the router (everything except static files).
pub fn build_api(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws", get(ws::ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/vapid",
            get({
                let state = state.clone();
                move || {
                    let key = state.push.public_b64.clone();
                    async move { Json(serde_json::json!({ "publicKey": key })) }
                }
            }),
        )
        .with_state(state)
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let data_dir = PathBuf::from(env_or("RELAY_DATA_DIR", "/data"));
    let static_dir = PathBuf::from(env_or("RELAY_STATIC_DIR", "./static"));
    let port: u16 = env_or("RELAY_PORT", "8080").parse().expect("RELAY_PORT must be a port number");
    let subject = env_or("RELAY_VAPID_SUBJECT", "mailto:fleet-relay@muveeai.com");

    let vapid_key = match std::env::var("RELAY_VAPID_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            let k = Push::generate_private_key();
            log::warn!("no RELAY_VAPID_KEY set — generated an ephemeral VAPID key; push subscriptions will break on restart. To pin it set:");
            log::warn!("  RELAY_VAPID_KEY={k}");
            k
        }
    };
    let push = Push::new(vapid_key, subject, &data_dir).expect("init web push");
    log::info!("VAPID public key: {}", push.public_b64);

    let harmony = HarmonyPush::from_env();
    match &harmony {
        Some(_) => log::info!("HarmonyOS Push Kit channel enabled (RELAY_HARMONY_* set)"),
        None => log::info!(
            "HarmonyOS Push Kit channel disabled — set RELAY_HARMONY_CLIENT_ID / \
             RELAY_HARMONY_CLIENT_SECRET / RELAY_HARMONY_PROJECT_ID to enable"
        ),
    }

    let state = Arc::new(AppState { registry: Registry::default(), push, harmony });

    let index = static_dir.join("index.html");
    let static_service = ServeDir::new(&static_dir).not_found_service(ServeFile::new(index));

    let app = build_api(state).fallback_service(static_service);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    log::info!("fleet-relay listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            log::info!("shutting down");
        })
        .await
        .expect("serve");
}
