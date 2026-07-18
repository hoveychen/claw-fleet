use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let address =
        std::env::var("FLEET_CLOUD_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|error| panic!("failed to bind {address}: {error}"));
    eprintln!("fleet-cloud-api listening on {address}");
    axum::serve(listener, fleet_cloud_api::app())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("fleet-cloud-api server failed");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
