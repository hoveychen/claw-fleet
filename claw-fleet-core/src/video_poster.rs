//! Poster frames for video artifacts, extracted on the host.
//!
//! One source for every client: the desktop and browser cards load it as
//! `?poster=1` on the artifact blob URL, the phone gets it squeezed further
//! through the relay's `library_thumb`. Before this the desktop grabbed a
//! frame in the webview from a ranged read of the video and the phone had no
//! poster at all.
//!
//! Extraction uses `ffmpeg` (any platform, and its `thumbnail` filter skips
//! the black lead-in frame many renders start on), or, on a Mac without it,
//! the system's own `qlmanage -t` (Quick Look, backed by AVFoundation, so no
//! third-party install is needed). With neither there is no poster and the card keeps its
//! icon — a poster is decoration, never an error the user sees.
//!
//! Results are cached under `~/.fleet/cache/video-posters/<id>/`, keyed by the
//! artifact's current version and size so a re-published video is re-grabbed.
//! A video no extractor could read leaves a `.none` marker, so a broken file
//! costs one attempt rather than one per card render.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::artifacts::Artifact;

/// Longest edge of the stored poster. Big enough for the desktop's widest
/// card at 2x, small enough that a folder of them is cheap to list.
const POSTER_EDGE: u32 = 512;
/// Size the stored JPEG is squeezed toward, and never exceeded while
/// `POSTER_MIN_DIM` still allows shrinking.
const POSTER_TARGET_BYTES: usize = 48 * 1024;
const POSTER_HARD_CAP_BYTES: usize = 96 * 1024;
const POSTER_MIN_DIM: u32 = 160;
/// An extractor that has not answered by then is killed: a pathological file
/// must not pin a protocol thread.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(20);
/// Quick Look answers a real clip in ~0.2s but never exits on a file it
/// cannot read (measured: still running after 9 minutes on a junk `.mp4`),
/// so its leash is short.
const QLMANAGE_TIMEOUT: Duration = Duration::from_secs(8);
/// Concurrent extractions. A cold folder of a hundred videos asks for every
/// poster at once; decoding them all in parallel would starve the machine.
const MAX_CONCURRENT: usize = 3;

/// The poster JPEG for a video artifact's current version, or `None` when it
/// is not a video, no extractor is installed, or the file would not decode.
/// `Err` only for an unknown artifact.
pub fn poster_jpeg(id: &str) -> Result<Option<Vec<u8>>, String> {
    let artifact = crate::artifacts::get(id)?;
    let Some(root) = crate::artifacts::artifacts_dir() else {
        return Ok(None);
    };
    let Some(cache) = cache_dir() else {
        return Ok(None);
    };
    Ok(poster_for(&root, &cache, &artifact, &Extractors::detect()))
}

/// [`poster_jpeg`] for an artifact already loaded from the store at `root`
/// (the library summary worker, which runs against explicit roots).
pub fn poster_jpeg_for(root: &Path, artifact: &Artifact) -> Option<Vec<u8>> {
    poster_for(root, &cache_dir()?, artifact, &Extractors::detect())
}

/// [`poster_jpeg`] answered as blob bytes, for the two `?poster=1` surfaces
/// (the desktop's `fleet-artifact://` protocol and `fleet serve`'s
/// `/artifact_blob`). A missing poster is an error, which both answer as 404
/// so the card falls back to its icon.
pub fn read_poster(id: &str) -> Result<crate::artifacts::ArtifactBytes, String> {
    let bytes = poster_jpeg(id)?.ok_or_else(|| format!("artifact '{id}' has no poster"))?;
    Ok(crate::artifacts::ArtifactBytes {
        etag: format!("\"poster-{}-{}\"", id, bytes.len()),
        total_size: bytes.len() as u64,
        bytes,
        mime: "image/jpeg".to_string(),
        range: None,
    })
}

fn cache_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("cache").join("video-posters"))
}

/// The tools found on this host, resolved once per process: a desktop app's
/// PATH usually lacks Homebrew, so the usual install locations are checked
/// too.
#[derive(Clone, Debug, Default)]
pub(crate) struct Extractors {
    ffmpeg: Option<PathBuf>,
    qlmanage: Option<PathBuf>,
}

impl Extractors {
    fn detect() -> Self {
        static FOUND: OnceLock<Extractors> = OnceLock::new();
        FOUND
            .get_or_init(|| Extractors {
                ffmpeg: find_binary(
                    "ffmpeg",
                    &["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"],
                ),
                qlmanage: if cfg!(target_os = "macos") {
                    Some(PathBuf::from("/usr/bin/qlmanage")).filter(|p| p.exists())
                } else {
                    None
                },
            })
            .clone()
    }

    fn any(&self) -> bool {
        self.ffmpeg.is_some() || self.qlmanage.is_some()
    }
}

fn find_binary(name: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    crate::process_util::which(name)
        .map(PathBuf::from)
        .or_else(|| fallbacks.iter().map(PathBuf::from).find(|p| p.exists()))
}

fn is_video(artifact: &Artifact) -> bool {
    artifact.kind == "video"
        || artifact
            .mime
            .split(';')
            .next()
            .is_some_and(|m| m.trim().to_ascii_lowercase().starts_with("video/"))
}

/// The cached-or-extracted poster. Split from [`poster_jpeg`] so tests can
/// point it at a scratch store, cache and tool set.
pub(crate) fn poster_for(
    root: &Path,
    cache: &Path,
    artifact: &Artifact,
    tools: &Extractors,
) -> Option<Vec<u8>> {
    if !is_video(artifact) {
        return None;
    }
    let dir = cache.join(&artifact.id);
    let stem = format!("{}-{}", artifact.current_version, artifact.size_bytes);
    let hit = dir.join(format!("{stem}.jpg"));
    let miss = dir.join(format!("{stem}.none"));
    if let Ok(bytes) = std::fs::read(&hit) {
        return Some(bytes);
    }
    if miss.exists() {
        return None;
    }
    // Nothing to try with — and nothing to remember either, so installing
    // ffmpeg later is picked up on the next launch.
    if !tools.any() {
        return None;
    }

    let _slot = Slot::acquire();
    // Another request may have filled the cache while this one queued.
    if let Ok(bytes) = std::fs::read(&hit) {
        return Some(bytes);
    }
    let video = crate::artifacts::blob_path(root, artifact);
    let poster = extract(&video, tools).and_then(squeeze);
    let _ = std::fs::create_dir_all(&dir);
    match &poster {
        Some(bytes) => {
            // Drop posters of older versions before writing the new one.
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
            write_atomic(&hit, bytes);
        }
        None => write_atomic(&miss, b""),
    }
    poster
}

fn extract(video: &Path, tools: &Extractors) -> Option<Vec<u8>> {
    if !video.is_file() {
        return None;
    }
    // Quick Look is the stand-in for a missing ffmpeg, not a second opinion:
    // a file ffmpeg cannot read is broken, and Quick Look would only hang on
    // it until the timeout.
    match (&tools.ffmpeg, &tools.qlmanage) {
        (Some(ffmpeg), _) => extract_ffmpeg(ffmpeg, video),
        (None, Some(qlmanage)) => extract_qlmanage(qlmanage, video),
        (None, None) => None,
    }
}

/// The most representative of the first 30 frames (ffmpeg's `thumbnail`
/// filter), scaled to [`POSTER_EDGE`], as JPEG on stdout. A clip shorter than
/// 30 frames still yields one: the filter flushes at end of stream.
fn extract_ffmpeg(ffmpeg: &Path, video: &Path) -> Option<Vec<u8>> {
    let filter = format!(
        "thumbnail=30,scale='if(gt(iw,ih),min({e},iw),-2)':'if(gt(iw,ih),-2,min({e},ih))'",
        e = POSTER_EDGE
    );
    let mut cmd = crate::process_util::command(ffmpeg);
    cmd.args(["-nostdin", "-v", "error", "-i"])
        .arg(video)
        .args(["-vf", &filter, "-frames:v", "1", "-f", "image2", "-c:v", "mjpeg"])
        .args(["-q:v", "4", "pipe:1"]);
    run(cmd, FFMPEG_TIMEOUT).filter(|out| !out.is_empty())
}

/// Quick Look's thumbnail of the video — AVFoundation underneath, so it reads
/// whatever QuickTime can. It writes `<name>.png` into the output directory.
fn extract_qlmanage(qlmanage: &Path, video: &Path) -> Option<Vec<u8>> {
    let out_dir = scratch_dir()?;
    let mut cmd = crate::process_util::command(qlmanage);
    cmd.args(["-t", "-s", &POSTER_EDGE.to_string(), "-o"])
        .arg(&out_dir)
        .arg(video);
    let png = run(cmd, QLMANAGE_TIMEOUT).and_then(|_| {
        let name = video.file_name()?.to_string_lossy().to_string();
        std::fs::read(out_dir.join(format!("{name}.png"))).ok()
    });
    let _ = std::fs::remove_dir_all(&out_dir);
    png
}

fn scratch_dir() -> Option<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "fleet-poster-{}-{nanos}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Run to completion under `timeout`; stdout on a zero exit.
fn run(mut cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Drain stdout on a thread: a poster bigger than the pipe buffer would
    // otherwise block the child and look like a timeout.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().ok()?;
    status.filter(|s| s.success()).map(|_| out)
}

/// Re-encode whatever the extractor produced (ffmpeg's JPEG, Quick Look's
/// PNG) into the one stored shape: a JPEG under the size caps.
fn squeeze(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let (out, mime) = fleet_image::downscale_image(
        bytes,
        "application/octet-stream",
        POSTER_TARGET_BYTES,
        POSTER_HARD_CAP_BYTES,
        POSTER_MIN_DIM,
    );
    (mime == "image/jpeg").then_some(out)
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// A counting semaphore over [`MAX_CONCURRENT`] extraction slots.
struct Slot;

static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

impl Slot {
    fn acquire() -> Slot {
        let (lock, cvar) = &SLOTS;
        let mut busy = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *busy >= MAX_CONCURRENT {
            busy = cvar.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (lock, cvar) = &SLOTS;
        let mut busy = lock.lock().unwrap_or_else(|e| e.into_inner());
        *busy = busy.saturating_sub(1);
        cvar.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_artifact(root: &Path, id: &str, bytes: &[u8]) -> Artifact {
        let mut a: Artifact = serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "clip.mp4",
            "title": "clip",
            "mime": "video/mp4",
            "kind": "video",
            "sizeBytes": bytes.len(),
            "currentVersion": "v1",
            "workspacePath": "/tmp/ws",
            "workspaceName": "ws",
            "sourcePath": "",
            "createdMs": 0,
        }))
        .expect("artifact fixture");
        a.size_bytes = bytes.len() as u64;
        let blob = crate::artifacts::blob_path(root, &a);
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::write(&blob, bytes).unwrap();
        a
    }

    /// A tiny real clip made by the host's ffmpeg, or `None` when there is
    /// none — the extraction tests then have nothing to say.
    fn make_clip(dir: &Path, tools: &Extractors) -> Option<Vec<u8>> {
        let ffmpeg = tools.ffmpeg.as_ref()?;
        let out = dir.join("src.mp4");
        let ok = crate::process_util::command(ffmpeg)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=size=640x360:rate=10:duration=1")
            .args(["-pix_fmt", "yuv420p", "-y"])
            .arg(&out)
            .status()
            .ok()?
            .success();
        ok.then(|| std::fs::read(&out).ok()).flatten()
    }

    #[test]
    fn non_video_has_no_poster() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = video_artifact(tmp.path(), "a1", b"not really");
        a.kind = "image".into();
        a.mime = "image/png".into();
        let cache = tmp.path().join("cache");
        assert!(poster_for(tmp.path(), &cache, &a, &Extractors::detect()).is_none());
        assert!(!cache.exists(), "nothing is cached for a non-video");
    }

    #[test]
    fn no_extractor_degrades_to_none_without_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let a = video_artifact(tmp.path(), "a2", b"whatever");
        let cache = tmp.path().join("cache");
        assert!(poster_for(tmp.path(), &cache, &a, &Extractors::default()).is_none());
        assert!(
            !cache.join("a2").exists(),
            "no marker, so a later-installed ffmpeg is tried"
        );
    }

    #[test]
    fn undecodable_video_is_remembered_as_a_miss() {
        let tools = Extractors::detect();
        if !tools.any() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let a = video_artifact(tmp.path(), "a3", b"definitely not an mp4");
        let cache = tmp.path().join("cache");
        assert!(poster_for(tmp.path(), &cache, &a, &tools).is_none());
        assert!(cache.join("a3").join("v1-21.none").exists());
    }

    #[test]
    fn extracts_and_caches_a_jpeg_poster() {
        let tools = Extractors::detect();
        let tmp = tempfile::tempdir().unwrap();
        let Some(clip) = make_clip(tmp.path(), &tools) else {
            return;
        };
        let a = video_artifact(tmp.path(), "a4", &clip);
        let cache = tmp.path().join("cache");
        let first = poster_for(tmp.path(), &cache, &a, &tools).expect("a poster");
        assert_eq!(&first[..2], &[0xFF, 0xD8], "stored as JPEG");
        assert!(first.len() <= POSTER_HARD_CAP_BYTES);
        let img = image::load_from_memory(&first).unwrap();
        assert!(img.width() <= POSTER_EDGE && img.height() <= POSTER_EDGE);
        // Second read is the cached file, even with no tools at all.
        let again = poster_for(tmp.path(), &cache, &a, &Extractors::default()).unwrap();
        assert_eq!(first, again);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quick_look_alone_yields_a_poster() {
        let tools = Extractors::detect();
        let tmp = tempfile::tempdir().unwrap();
        let Some(clip) = make_clip(tmp.path(), &tools) else {
            return;
        };
        let ql_only = Extractors {
            ffmpeg: None,
            qlmanage: tools.qlmanage.clone(),
        };
        if !ql_only.any() {
            return;
        }
        let a = video_artifact(tmp.path(), "a5", &clip);
        let poster = poster_for(tmp.path(), &tmp.path().join("cache"), &a, &ql_only)
            .expect("Quick Look poster");
        assert_eq!(&poster[..2], &[0xFF, 0xD8]);
    }
}
