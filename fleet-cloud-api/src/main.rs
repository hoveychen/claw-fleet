use std::sync::Arc;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL is required");
    let store = fleet_cloud_api::store::PgTaskStore::connect(&database_url)
        .await
        .expect("failed to connect to Postgres");
    store.migrate().await.expect("database migration failed");
    let webhook_dispatcher = fleet_cloud_api::webhook::PgWebhookDispatcher::new(
        store.pool().clone(),
        Arc::new(fleet_cloud_api::webhook::ReqwestWebhookTransport::default()),
    );
    tokio::spawn(async move {
        loop {
            if let Err(error) = webhook_dispatcher.dispatch_once(50).await {
                eprintln!("fleet-cloud webhook dispatcher error: {error}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    let address =
        std::env::var("FLEET_CLOUD_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    eprintln!("fleet-cloud-api listening on {address}");
    let router = match std::env::var("FLEET_CLOUD_EMBED_SECRET") {
        Ok(secret) if !secret.is_empty() => fleet_cloud_api::app_with_store_and_embed(
            Arc::new(store),
            Arc::new(fleet_cloud_api::embed::EmbedTokenVerifier::new(
                secret.as_bytes(),
            )),
        ),
        _ => fleet_cloud_api::app_with_store(Arc::new(store)),
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("fleet-cloud-api server failed");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
