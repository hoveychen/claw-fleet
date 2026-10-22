//! A bounded byte pipe between a producer thread and an HTTP response body.
//!
//! Exists so `fleet serve` can send a zip while it is still being packed,
//! instead of packing it into a temporary file first. That temp file cost a
//! second on-disk copy as large as the export (gigabytes for a project of
//! renders, per concurrent download, in a cloud container whose disk is the
//! scarce thing), a wait before the first byte, and a stray file whenever the
//! process died mid-export.
//!
//! The pipe is a `sync_channel` of fixed-size chunks, so at most
//! `depth + 2` chunks are ever in flight (the queue, the one the writer is
//! filling and the one the reader is draining). A slow client therefore stalls
//! the producer — backpressure — rather than letting the archive pile up in
//! memory. When the reader is dropped (the response finished or the client
//! went away and tiny_http gave up on it), the producer's next write fails
//! with `BrokenPipe`, which unwinds it out of whatever it was copying.

use std::io::{self, Read, Write};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::JoinHandle;

/// Chunk size handed across the channel: the same 64 KiB the zip writer
/// copies with, so one member read is one send.
pub const CHUNK: usize = 64 * 1024;
/// Chunks the channel holds before the producer blocks.
pub const DEPTH: usize = 4;

type Msg = Result<Vec<u8>, String>;

/// Write half; lives on the producer thread.
pub struct PipeWriter {
    tx: SyncSender<Msg>,
    buf: Vec<u8>,
    chunk: usize,
}

/// Read half; becomes the response body.
pub struct PipeReader {
    rx: Receiver<Msg>,
    cur: Vec<u8>,
    pos: usize,
}

pub fn pipe(chunk: usize, depth: usize) -> (PipeWriter, PipeReader) {
    let (tx, rx) = sync_channel(depth);
    (
        PipeWriter { tx, buf: Vec::with_capacity(chunk), chunk },
        PipeReader { rx, cur: Vec::new(), pos: 0 },
    )
}

fn reader_gone() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "stream reader went away")
}

impl PipeWriter {
    fn send_buf(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let full = std::mem::replace(&mut self.buf, Vec::with_capacity(self.chunk));
        self.tx.send(Ok(full)).map_err(|_| reader_gone())
    }

    /// Tell the reader the stream failed. Whatever was buffered is dropped:
    /// the output is incomplete either way, and the reader turns this into an
    /// I/O error instead of a clean end of stream.
    fn fail(self, message: String) {
        let _ = self.tx.send(Err(message));
    }
}

impl Write for PipeWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let room = self.chunk - self.buf.len();
        let n = data.len().min(room);
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() >= self.chunk {
            self.send_buf()?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_buf()
    }
}

impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.cur.len() {
            match self.rx.recv() {
                Ok(Ok(chunk)) => {
                    self.cur = chunk;
                    self.pos = 0;
                }
                Ok(Err(message)) => return Err(io::Error::other(message)),
                // Producer finished (or panicked) and dropped its sender.
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.cur.len() - self.pos);
        out[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Run `produce` on its own thread, writing into a bounded pipe, and return
/// the read half plus the thread's handle.
///
/// `produce` returning `Err` is forwarded to the reader as an I/O error. The
/// caller should join the handle once it has dropped the reader: the producer
/// is then guaranteed to be finished or about to fail on its next write, so
/// the join never waits on a client.
pub fn spawn<F>(name: &str, produce: F) -> io::Result<(PipeReader, JoinHandle<Result<(), String>>)>
where
    F: FnOnce(&mut PipeWriter) -> Result<(), String> + Send + 'static,
{
    let (mut writer, reader) = pipe(CHUNK, DEPTH);
    let handle = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || match produce(&mut writer).and_then(|()| writer.flush().map_err(|e| e.to_string())) {
            Ok(()) => Ok(()),
            Err(e) => {
                writer.fail(e.clone());
                Err(e)
            }
        })?;
    Ok((reader, handle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bytes_cross_in_order_across_many_chunks() {
        let body: Vec<u8> = (0..CHUNK * 3 + 17).map(|i| (i % 253) as u8).collect();
        let expected = body.clone();
        let (mut reader, handle) = spawn("t", move |w| w.write_all(&body).map_err(|e| e.to_string())).unwrap();
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert_eq!(got, expected);
        handle.join().unwrap().unwrap();
    }

    #[test]
    fn a_producer_error_reaches_the_reader_as_an_io_error() {
        let (mut reader, handle) = spawn("t", |w| {
            w.write_all(b"partial").map_err(|e| e.to_string())?;
            Err("disk vanished".to_string())
        })
        .unwrap();
        let mut got = Vec::new();
        let err = reader.read_to_end(&mut got).unwrap_err();
        assert!(err.to_string().contains("disk vanished"), "{err}");
        assert_eq!(handle.join().unwrap(), Err("disk vanished".to_string()));
    }

    #[test]
    fn a_slow_reader_blocks_the_producer_at_the_bound() {
        let (written_tx, written_rx) = std::sync::mpsc::channel::<usize>();
        let (reader, handle) = spawn("t", move |w| {
            let chunk = vec![0u8; CHUNK];
            for i in 0..100 {
                w.write_all(&chunk).map_err(|e| e.to_string())?;
                let _ = written_tx.send(i + 1);
            }
            Ok(())
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        let progressed = written_rx.try_iter().last().unwrap_or(0);
        // DEPTH queued + one in the writer's own buffer is the most it can
        // get ahead with nobody reading.
        assert!(progressed <= DEPTH + 1, "producer ran {progressed} chunks ahead");
        drop(reader);
        let res = handle.join().unwrap();
        assert!(res.is_err(), "producer must stop once the reader is gone");
    }

    #[test]
    fn dropping_the_reader_unblocks_a_producer_waiting_on_a_full_queue() {
        let (reader, handle) = spawn("t", |w| loop {
            w.write_all(&[1u8; 4096]).map_err(|e| e.to_string())?;
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        drop(reader);
        let err = handle.join().unwrap().unwrap_err();
        assert!(err.contains("reader went away"), "{err}");
    }
}
