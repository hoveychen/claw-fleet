//! Tunnel provider abstraction for exposing the local mobile-access HTTP
//! server to the public internet.
//!
//! The default provider is [`CloudflareTunnelProvider`], which spawns
//! `cloudflared tunnel --url http://localhost:<port>` to expose the port via a
//! random `*.trycloudflare.com` URL — no Cloudflare account needed. If
//! `cloudflared` is not found on the system, it is automatically downloaded to
//! `~/.fleet/cloudflared` on first use (~18 MB).
//!
//! Future providers (localtunnel, OpenFrp, …) plug in by implementing the
//! [`TunnelProvider`] + [`TunnelHandle`] traits below.

use std::io::BufRead;
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

// ── Provider abstraction ────────────────────────────────────────────────────

/// A backend that can establish a public-internet tunnel to a local port.
pub trait TunnelProvider: Send + Sync {
    /// Stable identifier (e.g. `"cloudflare"`) used in logs and UI.
    fn name(&self) -> &'static str;

    /// True when the provider's runtime dependencies (binaries etc.) are
    /// already installed and no download is required.
    fn is_available(&self) -> bool;

    /// Bring up the tunnel forwarding the public URL to
    /// `http://127.0.0.1:<local_port>`.
    ///
    /// `on_phase` is invoked with stable identifiers (`"downloading"`,
    /// `"tunnel"`, …) so the UI can render setup progress. `on_progress`
    /// reports `(downloaded, total)` byte counts while the provider's binary
    /// is being fetched; not all providers download a binary.
    fn start(
        &self,
        local_port: u16,
        on_phase: Option<PhaseFn>,
        on_progress: Option<ProgressFn>,
    ) -> Result<BoxedTunnelHandle, TunnelError>;
}

/// A live tunnel. Dropping the handle stops the tunnel.
pub trait TunnelHandle: Send {
    /// Public HTTPS URL routing to the local port.
    fn url(&self) -> &str;
    /// Stop the tunnel and release resources. Idempotent.
    fn stop(&mut self);
}

/// Boxed trait object alias used across the desktop crate.
pub type BoxedTunnelHandle = Box<dyn TunnelHandle + Send>;

/// Boxed provider alias — used when the caller wants an ordered fallback list
/// without committing to a concrete provider type.
pub type BoxedTunnelProvider = Box<dyn TunnelProvider + Send + Sync>;

/// Phase callback type: receives identifiers like `"downloading"` / `"tunnel"`.
pub type PhaseFn = Box<dyn Fn(&'static str) + Send>;

/// Progress callback type: receives `(downloaded_bytes, total_bytes)`.
pub type ProgressFn = Box<dyn Fn(u64, u64) + Send>;

/// Resolve the ordered list of providers to try given a user preference.
///
/// - `Some("cloudflare")` / `Some("localtunnel")` pin a single provider — no
///   fallback, the caller fails out if it doesn't work.
/// - `None` or `Some("auto")` returns the default fallback chain
///   (cloudflared first, localtunnel second). Future revisions will reorder
///   based on detected region (P3: OpenFrp first for users in China).
pub fn select_providers(preference: Option<&str>) -> Vec<BoxedTunnelProvider> {
    match preference.unwrap_or("auto") {
        "cloudflare" => vec![Box::new(CloudflareTunnelProvider::new())],
        "localtunnel" => vec![Box::new(LocaltunnelProvider::new())],
        _ => vec![
            Box::new(CloudflareTunnelProvider::new()),
            Box::new(LocaltunnelProvider::new()),
        ],
    }
}

// ── Common error type ───────────────────────────────────────────────────────

/// Errors that can occur when starting a tunnel.
#[derive(Debug)]
pub enum TunnelError {
    /// Failed to download the provider's required binary.
    DownloadFailed(String),
    /// Failed to spawn the provider's process.
    SpawnFailed(String),
    /// Could not parse the public URL from provider output.
    /// Contains the last few lines of stderr/stdout for diagnostics.
    UrlParseTimeout(String),
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TunnelError::DownloadFailed(e) => write!(f, "failed to download tunnel binary: {e}"),
            TunnelError::SpawnFailed(e) => write!(f, "failed to start tunnel process: {e}"),
            TunnelError::UrlParseTimeout(output) => write!(
                f,
                "timed out waiting for tunnel URL. provider output:\n{output}"
            ),
        }
    }
}

// ── Cloudflare provider ─────────────────────────────────────────────────────

/// Provider that exposes the local port via Cloudflare's Quick Tunnel
/// (`*.trycloudflare.com`). No Cloudflare account required.
pub struct CloudflareTunnelProvider;

impl CloudflareTunnelProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CloudflareTunnelProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelProvider for CloudflareTunnelProvider {
    fn name(&self) -> &'static str {
        "cloudflare"
    }

    fn is_available(&self) -> bool {
        is_cloudflared_available()
    }

    fn start(
        &self,
        local_port: u16,
        on_phase: Option<PhaseFn>,
        on_progress: Option<ProgressFn>,
    ) -> Result<BoxedTunnelHandle, TunnelError> {
        if let Some(ref cb) = on_phase {
            cb("downloading");
        }
        let binary = find_or_download_cloudflared_with_progress(on_progress)?;
        if let Some(ref cb) = on_phase {
            cb("tunnel");
        }
        let handle = CloudflareTunnel::start_with_binary(&binary, local_port)?;
        Ok(Box::new(handle))
    }
}

/// A running Cloudflare Quick Tunnel.
pub struct CloudflareTunnel {
    process: Child,
    public_url: String,
}

impl CloudflareTunnel {
    /// Start a Quick Tunnel using a specific binary path.
    pub fn start_with_binary(binary: &str, local_port: u16) -> Result<Self, TunnelError> {
        eprintln!("[tunnel] starting: {binary} tunnel --url http://127.0.0.1:{local_port}");

        let mut child = Command::new(binary)
            .args([
                "tunnel",
                "--protocol", "http2",
                "--url",
                &format!("http://127.0.0.1:{local_port}"),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TunnelError::SpawnFailed(format!("{e} (binary: {binary})")))?;

        // cloudflared prints the tunnel URL to stderr. Some Windows AV / wrapper
        // setups relay lines to stdout instead, so we tail both.
        let stderr = child.stderr.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (url_tx, url_rx) = mpsc::channel();
        let log_lines: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let spawn_tail = |stream: Box<dyn std::io::Read + Send>, tag: &'static str| {
            let tx = url_tx.clone();
            let buf = log_lines.clone();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stream);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    eprintln!("[cloudflared {tag}] {line}");
                    buf.lock().unwrap().push(format!("[{tag}] {line}"));
                    if let Some(url) = extract_tunnel_url(&line) {
                        let _ = tx.send(url);
                    }
                }
            });
        };
        spawn_tail(Box::new(stderr), "stderr");
        spawn_tail(Box::new(stdout), "stdout");
        // The closure keeps a clone; drop ours so the channel can close when
        // both reader threads finish (i.e. child closed both pipes).
        drop(url_tx);

        match url_rx.recv_timeout(Duration::from_secs(30)) {
            Ok(public_url) => {
                eprintln!("[tunnel] public URL: {public_url}");
                Ok(CloudflareTunnel {
                    process: child,
                    public_url,
                })
            }
            Err(_) => {
                // Did the child already die? That usually means AV blocked the
                // binary, MOTW quarantine, missing permissions, or a crash —
                // much more useful than a generic 30s timeout.
                let exit = child.try_wait().ok().flatten();
                let _ = child.kill();
                let _ = child.wait();

                let lines = log_lines.lock().unwrap();
                let tail: String = if lines.is_empty() {
                    "(no output from cloudflared)".to_string()
                } else {
                    // Keep the full transcript, but cap to avoid huge payloads.
                    let slice = if lines.len() > 50 {
                        &lines[lines.len() - 50..]
                    } else {
                        &lines[..]
                    };
                    slice.join("\n")
                };
                let header = match exit {
                    Some(status) => format!("cloudflared exited early with {status}\n"),
                    None => String::new(),
                };
                Err(TunnelError::UrlParseTimeout(format!("{header}{tail}")))
            }
        }
    }

}

impl TunnelHandle for CloudflareTunnel {
    fn url(&self) -> &str {
        &self.public_url
    }

    fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Drop for CloudflareTunnel {
    fn drop(&mut self) {
        TunnelHandle::stop(self);
    }
}

// ── Find or download ────────────────────────────────────────────────────────

/// Find cloudflared on the system, or auto-download it to `~/.fleet/`, with an
/// optional progress callback for UI reporting.
pub fn find_or_download_cloudflared_with_progress(
    on_progress: Option<ProgressFn>,
) -> Result<String, TunnelError> {
    if let Some(path) = find_cloudflared() {
        return Ok(path);
    }

    eprintln!("[tunnel] cloudflared not found, downloading...");
    download_cloudflared(on_progress)
}

/// Check if `cloudflared` is already available and return the path.
fn find_cloudflared() -> Option<String> {
    #[cfg(unix)]
    let which = "which";
    #[cfg(not(unix))]
    let which = "where";

    match Command::new(which).arg("cloudflared").output() {
        Ok(output) if output.status.success() => {
            // Windows `where` can return multiple lines (one per PATHEXT hit) with CRLF.
            // Take the first non-empty line.
            let raw = String::from_utf8_lossy(&output.stdout).to_string();
            let first = raw.lines().map(str::trim).find(|s| !s.is_empty()).unwrap_or("");
            if !first.is_empty() {
                eprintln!("[tunnel] find: {which} -> {first}");
                return Some(first.to_string());
            }
            eprintln!("[tunnel] find: {which} succeeded but output was empty");
        }
        Ok(output) => {
            eprintln!("[tunnel] find: {which} exit {:?}", output.status.code());
        }
        Err(e) => {
            eprintln!("[tunnel] find: failed to spawn `{which}`: {e}");
        }
    }

    for path in ["/opt/homebrew/bin/cloudflared", "/usr/local/bin/cloudflared"] {
        if std::path::Path::new(path).exists() {
            eprintln!("[tunnel] find: {path}");
            return Some(path.to_string());
        }
    }

    if let Some(path) = fleet_cloudflared_path() {
        if path.exists() {
            eprintln!("[tunnel] find: cached {}", path.display());
            return Some(path.to_string_lossy().to_string());
        } else {
            eprintln!("[tunnel] find: no cached binary at {}", path.display());
        }
    }

    None
}

/// Path where we store the auto-downloaded cloudflared binary.
fn fleet_cloudflared_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| {
        #[cfg(windows)]
        { h.join(".fleet").join("cloudflared.exe") }
        #[cfg(not(windows))]
        { h.join(".fleet").join("cloudflared") }
    })
}

/// Download cloudflared binary to `~/.fleet/cloudflared`.
/// Accepts an optional progress callback `on_progress(downloaded, total)`.
pub fn download_cloudflared(on_progress: Option<ProgressFn>) -> Result<String, TunnelError> {
    let dest = fleet_cloudflared_path()
        .ok_or_else(|| TunnelError::DownloadFailed("cannot determine home directory".into()))?;

    // Create ~/.fleet/ if needed.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| TunnelError::DownloadFailed(format!("cannot create {}: {e}", parent.display())))?;
    }

    let url = download_url();
    let is_tgz = url.ends_with(".tgz");

    eprintln!("[tunnel] downloading from {url}");

    // Download using reqwest (blocking) with streaming for progress.
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| TunnelError::DownloadFailed(e.to_string()))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| TunnelError::DownloadFailed(e.to_string()))?;

    if !response.status().is_success() {
        return Err(TunnelError::DownloadFailed(format!(
            "HTTP {}",
            response.status()
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut buf = Vec::with_capacity(total_size as usize);

    let mut reader = response;
    loop {
        let mut chunk = vec![0u8; 64 * 1024]; // 64KB chunks
        match std::io::Read::read(&mut reader, &mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                downloaded += n as u64;
                if let Some(ref cb) = on_progress {
                    cb(downloaded, total_size);
                }
            }
            Err(e) => return Err(TunnelError::DownloadFailed(e.to_string())),
        }
    }

    eprintln!("[tunnel] downloaded {} bytes", buf.len());
    let bytes = buf;

    if is_tgz {
        // macOS/Linux: .tgz archive containing a single `cloudflared` binary.
        // Use the system `tar` command to preserve binary integrity (code signatures etc).
        let tgz_path = dest.with_extension("tgz");
        std::fs::write(&tgz_path, &bytes)
            .map_err(|e| TunnelError::DownloadFailed(format!("write tgz failed: {e}")))?;

        let parent = dest.parent().unwrap();
        let output = Command::new("tar")
            .args(["xzf"])
            .arg(&tgz_path)
            .arg("-C")
            .arg(parent)
            .output()
            .map_err(|e| TunnelError::DownloadFailed(format!("tar extract failed: {e}")))?;

        // Clean up the tgz.
        let _ = std::fs::remove_file(&tgz_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TunnelError::DownloadFailed(format!("tar failed: {stderr}")));
        }

        // Verify the binary exists.
        if !dest.exists() {
            return Err(TunnelError::DownloadFailed(
                "cloudflared binary not found after extraction".into(),
            ));
        }
    } else {
        // Linux/Windows: raw binary.
        std::fs::write(&dest, &bytes)
            .map_err(|e| TunnelError::DownloadFailed(format!("write failed: {e}")))?;
    }

    // Post-write verification — on Windows, SmartScreen / AV can quarantine
    // the exe between write and spawn, making failures opaque.
    match std::fs::metadata(&dest) {
        Ok(md) => eprintln!(
            "[tunnel] post-write: {} exists, {} bytes",
            dest.display(),
            md.len()
        ),
        Err(e) => eprintln!(
            "[tunnel] post-write: {} DISAPPEARED ({e})",
            dest.display()
        ),
    }

    // Make executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }

    // Remove macOS quarantine attribute so Gatekeeper doesn't block execution.
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(&dest)
            .output();
    }

    let path = dest.to_string_lossy().to_string();
    eprintln!("[tunnel] installed cloudflared to {path}");
    Ok(path)
}

/// Return the platform-specific download URL for cloudflared.
fn download_url() -> String {
    let base = "https://github.com/cloudflare/cloudflared/releases/latest/download";

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    { format!("{base}/cloudflared-darwin-arm64.tgz") }

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    { format!("{base}/cloudflared-darwin-amd64.tgz") }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    { format!("{base}/cloudflared-linux-amd64") }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    { format!("{base}/cloudflared-linux-arm64") }

    #[cfg(target_os = "windows")]
    { format!("{base}/cloudflared-windows-amd64.exe") }
}

// ── URL extraction ──────────────────────────────────────────────────────────

/// Extract a `https://...trycloudflare.com` URL from a cloudflared log line.
fn extract_tunnel_url(line: &str) -> Option<String> {
    for word in line.split_whitespace() {
        let word = word.trim_matches('|').trim();
        if word.starts_with("https://") && word.contains("trycloudflare.com") {
            return Some(word.to_string());
        }
    }
    // Also check for url= format
    if let Some(pos) = line.find("url=https://") {
        let start = pos + 4; // skip "url="
        let url_part = &line[start..];
        let end = url_part
            .find(|c: char| c.is_whitespace())
            .unwrap_or(url_part.len());
        let url = &url_part[..end];
        if url.contains("trycloudflare.com") {
            return Some(url.to_string());
        }
    }
    None
}

/// Check whether cloudflared is available on this system (without downloading).
pub fn is_cloudflared_available() -> bool {
    find_cloudflared().is_some()
}

// ── Localtunnel provider ────────────────────────────────────────────────────

/// Default upstream for the localtunnel allocation API.
const LOCALTUNNEL_HOST: &str = "https://localtunnel.me";

/// Provider that exposes the local port via the public localtunnel.me service
/// (`*.loca.lt`). No account needed, no binary to download — the protocol is
/// implemented in-process.
///
/// Protocol summary:
/// 1. `GET /?new` returns `{id, url, port, max_conn_count}`. `url` is the
///    public HTTPS URL, `port` is the TCP port on the upstream host that the
///    client must dial to receive incoming HTTP request bytes.
/// 2. The client maintains up to `max_conn_count` long-lived TCP connections
///    to `<host>:<port>`. Whenever the upstream forwards an inbound HTTP
///    request, it pipes raw bytes through one of those connections; the
///    client relays the bytes to the local server and pipes the response back.
/// 3. When a connection ends (request served, peer reset, etc.) the client
///    reconnects.
pub struct LocaltunnelProvider {
    base: String,
}

impl LocaltunnelProvider {
    pub fn new() -> Self {
        Self { base: LOCALTUNNEL_HOST.to_string() }
    }

    /// Override the allocation host (mainly for tests / self-hosted servers).
    pub fn with_base<S: Into<String>>(base: S) -> Self {
        Self { base: base.into() }
    }
}

impl Default for LocaltunnelProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(serde::Deserialize)]
struct LocaltunnelInfo {
    #[allow(dead_code)]
    id: String,
    port: u16,
    max_conn_count: Option<u32>,
    url: String,
}

impl TunnelProvider for LocaltunnelProvider {
    fn name(&self) -> &'static str {
        "localtunnel"
    }

    fn is_available(&self) -> bool {
        // Pure protocol implementation — nothing to install, always usable as
        // long as the network reaches localtunnel.me.
        true
    }

    fn start(
        &self,
        local_port: u16,
        on_phase: Option<PhaseFn>,
        _on_progress: Option<ProgressFn>,
    ) -> Result<BoxedTunnelHandle, TunnelError> {
        if let Some(ref cb) = on_phase {
            cb("tunnel");
        }

        let allocate_url = format!("{}/?new", self.base);
        eprintln!("[localtunnel] allocating: {allocate_url}");

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| TunnelError::SpawnFailed(format!("build http client: {e}")))?;

        let resp = client
            .get(&allocate_url)
            .send()
            .map_err(|e| TunnelError::SpawnFailed(format!("allocate request: {e}")))?;

        if !resp.status().is_success() {
            return Err(TunnelError::SpawnFailed(format!(
                "allocate returned HTTP {}",
                resp.status()
            )));
        }

        let info: LocaltunnelInfo = resp
            .json()
            .map_err(|e| TunnelError::SpawnFailed(format!("parse allocate JSON: {e}")))?;

        let upstream_host = parse_https_host(&info.url)
            .ok_or_else(|| TunnelError::SpawnFailed(format!("malformed URL: {}", info.url)))?
            .to_string();
        eprintln!(
            "[localtunnel] url={} upstream={}:{}",
            info.url, upstream_host, info.port
        );

        // Clamp the connection pool: server hint may be aggressive, but a
        // single user only needs a handful of parallel requests.
        let conn_count = info.max_conn_count.unwrap_or(10).clamp(1, 10) as usize;

        let shutdown = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::with_capacity(conn_count);
        for i in 0..conn_count {
            let host = upstream_host.clone();
            let port = info.port;
            let shutdown = shutdown.clone();
            let handle = std::thread::Builder::new()
                .name(format!("localtunnel-{i}"))
                .spawn(move || localtunnel_worker_loop(host, port, local_port, shutdown))
                .map_err(|e| TunnelError::SpawnFailed(format!("spawn worker {i}: {e}")))?;
            workers.push(handle);
        }

        Ok(Box::new(LocaltunnelHandle {
            public_url: info.url,
            shutdown,
            workers: Some(workers),
        }))
    }
}

/// A live localtunnel session. Dropping (or calling `stop`) signals every
/// worker thread to exit and joins them.
pub struct LocaltunnelHandle {
    public_url: String,
    shutdown: Arc<AtomicBool>,
    workers: Option<Vec<JoinHandle<()>>>,
}

impl TunnelHandle for LocaltunnelHandle {
    fn url(&self) -> &str {
        &self.public_url
    }

    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(workers) = self.workers.take() {
            for w in workers {
                let _ = w.join();
            }
        }
    }
}

impl Drop for LocaltunnelHandle {
    fn drop(&mut self) {
        TunnelHandle::stop(self);
    }
}

/// Extract the host portion of an `https://` URL — e.g.
/// `https://abc.loca.lt/foo` → `abc.loca.lt`.
fn parse_https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..end];
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// Continuously maintain one reverse-tunnel TCP connection to the localtunnel
/// upstream, reconnecting when an inbound request finishes or the dial fails.
/// Exits when `shutdown` flips to true (next iteration after the current
/// connection winds down).
fn localtunnel_worker_loop(
    host: String,
    remote_port: u16,
    local_port: u16,
    shutdown: Arc<AtomicBool>,
) {
    let addr_str = format!("{host}:{remote_port}");
    while !shutdown.load(Ordering::Relaxed) {
        let resolved: Option<SocketAddr> = addr_str
            .to_socket_addrs()
            .ok()
            .and_then(|mut it| it.next());
        let Some(remote_addr) = resolved else {
            eprintln!("[localtunnel] DNS resolve failed: {addr_str}");
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        match TcpStream::connect_timeout(&remote_addr, Duration::from_secs(10)) {
            Ok(remote) => {
                // Bound the read so the worker can periodically observe
                // `shutdown` even when the upstream keeps an idle connection
                // open.
                let _ = remote.set_read_timeout(Some(Duration::from_secs(60)));
                if let Err(e) = localtunnel_pipe_one(remote, local_port) {
                    eprintln!("[localtunnel] pipe error: {e}");
                }
            }
            Err(e) => {
                eprintln!("[localtunnel] connect {remote_addr} failed: {e}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    }
}

/// Bridge a single inbound localtunnel connection with the local HTTP server,
/// piping bytes both ways until either side closes.
fn localtunnel_pipe_one(remote: TcpStream, local_port: u16) -> std::io::Result<()> {
    let local_addr = SocketAddr::from(([127, 0, 0, 1], local_port));
    let local = TcpStream::connect_timeout(&local_addr, Duration::from_secs(5))?;

    let mut remote_a = remote.try_clone()?;
    let mut local_a = local.try_clone()?;
    let mut remote_b = remote;
    let mut local_b = local;

    let t1 = std::thread::spawn(move || {
        let _ = std::io::copy(&mut remote_a, &mut local_a);
        let _ = local_a.shutdown(Shutdown::Write);
    });
    let t2 = std::thread::spawn(move || {
        let _ = std::io::copy(&mut local_b, &mut remote_b);
        let _ = remote_b.shutdown(Shutdown::Write);
    });
    let _ = t1.join();
    let _ = t2.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_from_box() {
        let line =
            "2024-01-01T00:00:00Z INF |  https://foo-bar-baz.trycloudflare.com  |";
        assert_eq!(
            extract_tunnel_url(line),
            Some("https://foo-bar-baz.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn extract_url_from_registered_line() {
        let line = "2024-01-01T00:00:00Z INF Registered tunnel connection connIndex=0 url=https://abc-123.trycloudflare.com";
        assert_eq!(
            extract_tunnel_url(line),
            Some("https://abc-123.trycloudflare.com".to_string())
        );
    }

    #[test]
    fn no_url_in_line() {
        assert_eq!(extract_tunnel_url("some random log line"), None);
    }

    #[test]
    fn fleet_path_is_under_home() {
        let path = fleet_cloudflared_path();
        assert!(path.is_some());
        let p = path.unwrap();
        assert!(p.to_string_lossy().contains(".fleet"));
        assert!(p.to_string_lossy().contains("cloudflared"));
    }

    #[test]
    fn parse_https_host_extracts_hostname() {
        assert_eq!(parse_https_host("https://abc.loca.lt"), Some("abc.loca.lt"));
        assert_eq!(
            parse_https_host("https://abc.loca.lt/some/path"),
            Some("abc.loca.lt")
        );
        assert_eq!(
            parse_https_host("https://abc.loca.lt:443/p"),
            Some("abc.loca.lt:443")
        );
    }

    #[test]
    fn parse_https_host_rejects_non_https() {
        assert_eq!(parse_https_host("http://abc.loca.lt"), None);
        assert_eq!(parse_https_host("ftp://abc.loca.lt"), None);
        assert_eq!(parse_https_host("https:///nohost"), None);
    }

    #[test]
    fn localtunnel_provider_is_always_available() {
        let p = LocaltunnelProvider::new();
        assert_eq!(p.name(), "localtunnel");
        assert!(p.is_available());
    }
}
