use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use fleet_cloud_wire::runner::{ClientHello, CommandAck, RunnerFrame, ServerFrame};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tokio_tungstenite::{
    connect_async_tls_with_config, Connector, MaybeTlsStream, WebSocketStream,
};

use crate::journal::{CommandJournal, PersistResult};
use crate::outbox::EventOutbox;
use crate::supervisor::Supervisor;

pub async fn run_once(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    hello: ClientHello,
    journal: &mut CommandJournal,
    outbox: &EventOutbox,
) -> anyhow::Result<()> {
    run_once_inner(url, tls, hello, journal, outbox, None).await
}

pub async fn run_once_supervised(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    hello: ClientHello,
    journal: &mut CommandJournal,
    outbox: &EventOutbox,
    supervisor: &mut Supervisor,
) -> anyhow::Result<()> {
    run_once_inner(url, tls, hello, journal, outbox, Some(supervisor)).await
}

async fn run_once_inner(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    mut hello: ClientHello,
    journal: &mut CommandJournal,
    outbox: &EventOutbox,
    mut supervisor: Option<&mut Supervisor>,
) -> anyhow::Result<()> {
    let range = outbox.range()?;
    hello.outbox_first_sequence = range.0;
    hello.outbox_last_sequence = range.1;
    let (mut socket, _) = connect_cloud(url, tls, Duration::from_secs(15)).await?;
    send_runner(&mut socket, &RunnerFrame::ClientHello(hello)).await?;
    let server_hello = match next_server(&mut socket).await? {
        ServerFrame::ServerHello(hello) => hello,
        _ => anyhow::bail!("first Cloud frame must be server_hello"),
    };
    if let Some(sequence) = server_hello.request_outbox_from_sequence {
        send_outbox(&mut socket, outbox, sequence.saturating_sub(1)).await?;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(u64::from(
        server_hello.heartbeat_interval_seconds,
    )));
    loop {
        tokio::select! {
            message = socket.next() => {
                let message=message.ok_or_else(||anyhow::anyhow!("Cloud connection ended"))??;
                let frame=decode_server(message)?;
                match frame {
                    ServerFrame::Command(command)=>{
                        let persisted=journal.persist(&command)?;
                        let ack=CommandAck{
                            command_id:command.command_id.clone(),assignment_sequence:command.assignment_sequence,
                            status:journal.ack_status(&command.command_id)?,occurred_at:Utc::now(),result:None,error_code:None,
                        };
                        send_runner(&mut socket,&RunnerFrame::CommandAck(ack)).await?;
                        if persisted == PersistResult::Inserted {
                            if let Some(supervisor) = supervisor.as_deref_mut() {
                                let outcome=supervisor.execute(&command);
                                journal.mark_terminal(&command.command_id,outcome.status,outcome.result.as_ref())?;
                                send_runner(&mut socket,&RunnerFrame::CommandAck(CommandAck{
                                    command_id:command.command_id.clone(),assignment_sequence:command.assignment_sequence,
                                    status:outcome.status,occurred_at:Utc::now(),result:outcome.result,error_code:outcome.error_code,
                                })).await?;
                            }
                        }
                    }
                    ServerFrame::BatchAck{through_sequence}=>{outbox.acknowledge_through(through_sequence)?;}
                    ServerFrame::ServerHello(_)=>anyhow::bail!("duplicate server_hello"),
                }
            }
            _ = heartbeat.tick() => {
                let active_runs=if let Some(supervisor)=supervisor.as_deref_mut(){
                    supervisor.reconcile()?;
                    supervisor.active_runs()
                }else{0};
                send_runner(&mut socket,&RunnerFrame::Heartbeat{active_runs}).await?;
                let after=outbox.range()?.0.unwrap_or(1).saturating_sub(1);
                send_outbox(&mut socket,outbox,after).await?;
            }
        }
    }
}

async fn connect_cloud(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    timeout: Duration,
) -> anyhow::Result<(
    WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::handshake::client::Response,
)> {
    let request = url.into_client_request()?;
    tokio::time::timeout(
        timeout,
        connect_async_tls_with_config(request, None, false, Some(Connector::Rustls(tls))),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Cloud WebSocket connection timed out after {timeout:?}"))?
    .map_err(Into::into)
}

pub async fn run_forever(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    hello: ClientHello,
    journal: &mut CommandJournal,
    outbox: &EventOutbox,
    supervisor: &mut Supervisor,
) -> ! {
    let mut delay = Duration::from_secs(1);
    loop {
        if let Err(error) =
            run_once_supervised(url, tls.clone(), hello.clone(), journal, outbox, supervisor).await
        {
            tracing::warn!(%error,"Runner transport reconnecting");
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

async fn send_outbox<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    outbox: &EventOutbox,
    after: u64,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let events = outbox.batch_after(after, 200)?;
    if let Some(first) = events.first() {
        send_runner(
            socket,
            &RunnerFrame::EventBatch {
                first_sequence: first.sequence,
                events,
            },
        )
        .await?;
    }
    Ok(())
}

async fn send_runner<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    frame: &RunnerFrame,
) -> anyhow::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(serde_json::to_string(frame)?.into()))
        .await?;
    Ok(())
}

async fn next_server<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
) -> anyhow::Result<ServerFrame>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let message = socket
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("Cloud connection ended"))??;
    decode_server(message)
}

fn decode_server(message: Message) -> anyhow::Result<ServerFrame> {
    match message {
        Message::Text(text) => Ok(serde_json::from_str(&text)?),
        Message::Binary(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Message::Close(_) => anyhow::bail!("Cloud closed connection"),
        _ => anyhow::bail!("unexpected non-data WebSocket frame"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cloud_connect_times_out_when_peer_never_completes_tls() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        );
        let error = connect_cloud(
            &format!("wss://localhost:{}", address.port()),
            tls,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }
}
