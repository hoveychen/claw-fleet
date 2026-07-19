use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use fleet_cloud_wire::runner::{ClientHello, CommandAck, RunnerFrame, ServerFrame};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tokio_tungstenite::{connect_async_tls_with_config, Connector};

use crate::journal::CommandJournal;
use crate::outbox::EventOutbox;

pub async fn run_once(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    mut hello: ClientHello,
    journal: &mut CommandJournal,
    outbox: &EventOutbox,
) -> anyhow::Result<()> {
    let range = outbox.range()?;
    hello.outbox_first_sequence = range.0;
    hello.outbox_last_sequence = range.1;
    let request = url.into_client_request()?;
    let (mut socket, _) =
        connect_async_tls_with_config(request, None, false, Some(Connector::Rustls(tls))).await?;
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
                        journal.persist(&command)?;
                        let ack=CommandAck{
                            command_id:command.command_id.clone(),assignment_sequence:command.assignment_sequence,
                            status:journal.ack_status(&command.command_id)?,occurred_at:Utc::now(),result:None,error_code:None,
                        };
                        send_runner(&mut socket,&RunnerFrame::CommandAck(ack)).await?;
                    }
                    ServerFrame::BatchAck{through_sequence}=>{outbox.acknowledge_through(through_sequence)?;}
                    ServerFrame::ServerHello(_)=>anyhow::bail!("duplicate server_hello"),
                }
            }
            _ = heartbeat.tick() => {
                send_runner(&mut socket,&RunnerFrame::Heartbeat{active_runs:0}).await?;
                let after=outbox.range()?.0.unwrap_or(1).saturating_sub(1);
                send_outbox(&mut socket,outbox,after).await?;
            }
        }
    }
}

pub async fn run_forever(
    url: &str,
    tls: Arc<rustls::ClientConfig>,
    hello: ClientHello,
    journal: &mut CommandJournal,
    outbox: &EventOutbox,
) -> ! {
    let mut delay = Duration::from_secs(1);
    loop {
        if let Err(error) = run_once(url, tls.clone(), hello.clone(), journal, outbox).await {
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
