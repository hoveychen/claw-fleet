use std::sync::Arc;

use fleet_cloud_wire::runner::{RunnerFrame, ServerFrame};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::error::ApiError;
use crate::runner_gateway::registry;

pub fn tls_acceptor(
    server_chain: Vec<CertificateDer<'static>>,
    server_key: PrivateKeyDer<'static>,
    client_ca: CertificateDer<'static>,
) -> anyhow::Result<TlsAcceptor> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(client_ca)?;
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots)).build()?;
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(server_chain, server_key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

pub async fn serve(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    pool: PgPool,
) -> anyhow::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(error) = accept_connection(stream, acceptor, pool).await {
                tracing::warn!(%error, "Runner connection closed");
            }
        });
    }
}

pub async fn serve_one(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    pool: PgPool,
) -> anyhow::Result<()> {
    let (stream, _) = listener.accept().await?;
    accept_connection(stream, acceptor, pool).await
}

async fn accept_connection(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    pool: PgPool,
) -> anyhow::Result<()> {
    let tls = acceptor.accept(stream).await?;
    let fingerprint = peer_fingerprint(&tls)?;
    let mut socket = accept_async(tls).await?;
    let hello = match next_frame(&mut socket).await? {
        RunnerFrame::ClientHello(hello) => hello,
        _ => anyhow::bail!("first Runner frame must be client_hello"),
    };
    let runner_id = hello.runner_id.clone();
    let server_hello = registry::connect(&pool, &hello, &fingerprint).await?;
    send_frame(&mut socket, &ServerFrame::ServerHello(server_hello)).await?;
    registry::claim_available_commands(&pool, &runner_id).await?;
    for command in registry::pending_commands(&pool, &runner_id).await? {
        send_frame(&mut socket, &ServerFrame::Command(command)).await?;
    }
    let result = run_loop(&mut socket, &pool, &runner_id).await;
    let _ = sqlx::query("UPDATE runners SET status=CASE WHEN status='draining' THEN status ELSE 'offline' END,updated_at=now() WHERE id=$1 AND revoked_at IS NULL")
        .bind(&runner_id).execute(&pool).await;
    result
}

async fn run_loop(
    socket: &mut tokio_tungstenite::WebSocketStream<TlsStream<TcpStream>>,
    pool: &PgPool,
    runner_id: &str,
) -> anyhow::Result<()> {
    loop {
        match next_frame(socket).await? {
            RunnerFrame::Heartbeat { active_runs } => {
                registry::heartbeat(pool, runner_id, active_runs).await?;
                for command in registry::claim_available_commands(pool, runner_id).await? {
                    send_frame(socket, &ServerFrame::Command(command)).await?;
                }
            }
            RunnerFrame::EventBatch { events, .. } => {
                let through = registry::ingest_events(pool, runner_id, events).await?;
                send_frame(
                    socket,
                    &ServerFrame::BatchAck {
                        through_sequence: through,
                    },
                )
                .await?;
            }
            RunnerFrame::CommandAck(ack) => {
                registry::acknowledge_command(pool, runner_id, &ack).await?
            }
            RunnerFrame::ClientHello(_) => {
                return Err(ApiError::Validation("duplicate client_hello".into()).into())
            }
        }
    }
}

fn peer_fingerprint(stream: &TlsStream<TcpStream>) -> anyhow::Result<Vec<u8>> {
    let certificates = stream
        .get_ref()
        .1
        .peer_certificates()
        .ok_or_else(|| anyhow::anyhow!("mTLS client certificate required"))?;
    let leaf = certificates
        .first()
        .ok_or_else(|| anyhow::anyhow!("mTLS client certificate required"))?;
    Ok(Sha256::digest(leaf.as_ref()).to_vec())
}

async fn next_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<TlsStream<TcpStream>>,
) -> anyhow::Result<RunnerFrame> {
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(text) => return Ok(serde_json::from_str(&text)?),
            Message::Binary(bytes) => return Ok(serde_json::from_slice(&bytes)?),
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await?,
            Message::Close(_) => anyhow::bail!("Runner closed connection"),
            Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    anyhow::bail!("Runner connection ended")
}

async fn send_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<TlsStream<TcpStream>>,
    frame: &ServerFrame,
) -> anyhow::Result<()> {
    socket
        .send(Message::Text(serde_json::to_string(frame)?.into()))
        .await?;
    Ok(())
}
