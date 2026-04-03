pub mod account;
pub mod agent_source;
pub mod audit;
pub mod backend;
pub mod claude_analyze;
pub mod claude_source;
pub mod codex_source;
pub mod cursor;
pub mod daily_report;
pub mod hooks;
pub mod local_backend;
pub mod memory;
pub mod openclaw_source;
pub mod pattern_update;
pub mod remote;
pub mod search_index;
pub mod session;
pub mod skills;
pub mod tcc;
pub mod version_check;

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};
#[cfg(feature = "tts")]
use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Manager};
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;

use account::AccountInfo;
use backend::Backend;
use session::SessionInfo;

fn load_png_as_tray_icon(bytes: &[u8]) -> tauri::image::Image<'static> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("failed to decode tray icon PNG")
        .into_rgba8();
    let (w, h) = img.dimensions();
    tauri::image::Image::new_owned(img.into_raw(), w, h)
}

#[tauri::command]
fn get_log_path() -> String {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("claw-fleet-debug.log").to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

#[tauri::command]
fn check_app_version() -> version_check::VersionCheckResult {
    version_check::check_app_version()
}

fn log_debug(msg: &str) {
    if let Some(home) = dirs::home_dir() {
        let log_path = home.join(".claude").join("claw-fleet-debug.log");
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{timestamp}] {msg}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

// ── TTS via Microsoft Edge TTS ───────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
struct TtsVoice {
    name: String,
    lang: String,
    display_name: String,
    gender: String,
}

#[cfg(feature = "tts")]
static VOICES_CACHE: OnceLock<Vec<msedge_tts::voice::Voice>> = OnceLock::new();

#[cfg(feature = "tts")]
fn cached_voices() -> &'static Vec<msedge_tts::voice::Voice> {
    VOICES_CACHE.get_or_init(|| {
        msedge_tts::voice::get_voices_list().unwrap_or_default()
    })
}

#[cfg(feature = "tts")]
struct VoiceMeta {
    zh_name: &'static str,
    en_name: &'static str,
    gender_zh: &'static str,
    gender_en: &'static str,
}

#[cfg(feature = "tts")]
fn voice_display_map() -> &'static std::collections::HashMap<&'static str, VoiceMeta> {
    static MAP: OnceLock<std::collections::HashMap<&str, VoiceMeta>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = std::collections::HashMap::new();
        // zh-CN
        m.insert("zh-CN-XiaoxiaoNeural", VoiceMeta { zh_name: "晓晓", en_name: "Xiaoxiao", gender_zh: "女", gender_en: "Female" });
        m.insert("zh-CN-XiaoyiNeural", VoiceMeta { zh_name: "晓伊", en_name: "Xiaoyi", gender_zh: "女", gender_en: "Female" });
        m.insert("zh-CN-YunjianNeural", VoiceMeta { zh_name: "云健", en_name: "Yunjian", gender_zh: "男", gender_en: "Male" });
        m.insert("zh-CN-YunxiNeural", VoiceMeta { zh_name: "云希", en_name: "Yunxi", gender_zh: "男", gender_en: "Male" });
        m.insert("zh-CN-YunxiaNeural", VoiceMeta { zh_name: "云夏", en_name: "Yunxia", gender_zh: "男", gender_en: "Male" });
        m.insert("zh-CN-YunyangNeural", VoiceMeta { zh_name: "云扬", en_name: "Yunyang", gender_zh: "男", gender_en: "Male" });
        m.insert("zh-CN-liaoning-XiaobeiNeural", VoiceMeta { zh_name: "晓北 (东北话)", en_name: "Xiaobei (Northeastern)", gender_zh: "女", gender_en: "Female" });
        m.insert("zh-CN-shaanxi-XiaoniNeural", VoiceMeta { zh_name: "晓妮 (陕西话)", en_name: "Xiaoni (Shaanxi)", gender_zh: "女", gender_en: "Female" });
        // zh-HK
        m.insert("zh-HK-HiuGaaiNeural", VoiceMeta { zh_name: "曉佳", en_name: "HiuGaai", gender_zh: "女", gender_en: "Female" });
        m.insert("zh-HK-HiuMaanNeural", VoiceMeta { zh_name: "曉曼", en_name: "HiuMaan", gender_zh: "女", gender_en: "Female" });
        m.insert("zh-HK-WanLungNeural", VoiceMeta { zh_name: "雲龍", en_name: "WanLung", gender_zh: "男", gender_en: "Male" });
        // zh-TW
        m.insert("zh-TW-HsiaoChenNeural", VoiceMeta { zh_name: "曉臻", en_name: "HsiaoChen", gender_zh: "女", gender_en: "Female" });
        m.insert("zh-TW-YunJheNeural", VoiceMeta { zh_name: "雲哲", en_name: "YunJhe", gender_zh: "男", gender_en: "Male" });
        m.insert("zh-TW-HsiaoYuNeural", VoiceMeta { zh_name: "曉雨", en_name: "HsiaoYu", gender_zh: "女", gender_en: "Female" });
        // en-US
        m.insert("en-US-AvaNeural", VoiceMeta { zh_name: "Ava", en_name: "Ava", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-AndrewNeural", VoiceMeta { zh_name: "Andrew", en_name: "Andrew", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-EmmaNeural", VoiceMeta { zh_name: "Emma", en_name: "Emma", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-BrianNeural", VoiceMeta { zh_name: "Brian", en_name: "Brian", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-AnaNeural", VoiceMeta { zh_name: "Ana", en_name: "Ana", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-AriaNeural", VoiceMeta { zh_name: "Aria", en_name: "Aria", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-ChristopherNeural", VoiceMeta { zh_name: "Christopher", en_name: "Christopher", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-EricNeural", VoiceMeta { zh_name: "Eric", en_name: "Eric", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-GuyNeural", VoiceMeta { zh_name: "Guy", en_name: "Guy", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-JennyNeural", VoiceMeta { zh_name: "Jenny", en_name: "Jenny", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-MichelleNeural", VoiceMeta { zh_name: "Michelle", en_name: "Michelle", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-RogerNeural", VoiceMeta { zh_name: "Roger", en_name: "Roger", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-SteffanNeural", VoiceMeta { zh_name: "Steffan", en_name: "Steffan", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-AndrewMultilingualNeural", VoiceMeta { zh_name: "Andrew (多语言)", en_name: "Andrew (Multilingual)", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-AvaMultilingualNeural", VoiceMeta { zh_name: "Ava (多语言)", en_name: "Ava (Multilingual)", gender_zh: "女", gender_en: "Female" });
        m.insert("en-US-BrianMultilingualNeural", VoiceMeta { zh_name: "Brian (多语言)", en_name: "Brian (Multilingual)", gender_zh: "男", gender_en: "Male" });
        m.insert("en-US-EmmaMultilingualNeural", VoiceMeta { zh_name: "Emma (多语言)", en_name: "Emma (Multilingual)", gender_zh: "女", gender_en: "Female" });
        // en-GB
        m.insert("en-GB-LibbyNeural", VoiceMeta { zh_name: "Libby", en_name: "Libby", gender_zh: "女", gender_en: "Female" });
        m.insert("en-GB-MaisieNeural", VoiceMeta { zh_name: "Maisie", en_name: "Maisie", gender_zh: "女", gender_en: "Female" });
        m.insert("en-GB-RyanNeural", VoiceMeta { zh_name: "Ryan", en_name: "Ryan", gender_zh: "男", gender_en: "Male" });
        m.insert("en-GB-SoniaNeural", VoiceMeta { zh_name: "Sonia", en_name: "Sonia", gender_zh: "女", gender_en: "Female" });
        m.insert("en-GB-ThomasNeural", VoiceMeta { zh_name: "Thomas", en_name: "Thomas", gender_zh: "男", gender_en: "Male" });
        // en-AU
        m.insert("en-AU-NatashaNeural", VoiceMeta { zh_name: "Natasha", en_name: "Natasha", gender_zh: "女", gender_en: "Female" });
        m.insert("en-AU-WilliamMultilingualNeural", VoiceMeta { zh_name: "William (多语言)", en_name: "William (Multilingual)", gender_zh: "男", gender_en: "Male" });
        m
    })
}

#[cfg(feature = "tts")]
fn make_tts_voice(v: &msedge_tts::voice::Voice, locale: &str) -> TtsVoice {
    let short = v.short_name.clone().unwrap_or_else(|| v.name.clone());
    let map = voice_display_map();
    let is_zh = locale == "zh";

    let (display_name, gender) = if let Some(meta) = map.get(short.as_str()) {
        let name = if is_zh { meta.zh_name } else { meta.en_name };
        let g = if is_zh { meta.gender_zh } else { meta.gender_en };
        (name.to_string(), g.to_string())
    } else {
        // Fallback: extract name from ShortName (e.g. "en-IN-NeerjaNeural" → "Neerja")
        let fallback_name = short
            .rsplit('-')
            .next()
            .unwrap_or(&short)
            .trim_end_matches("Neural")
            .to_string();
        let g = v.gender.clone().unwrap_or_default();
        let gender = if is_zh {
            match g.as_str() { "Female" => "女".to_string(), "Male" => "男".to_string(), _ => g }
        } else {
            g
        };
        (fallback_name, gender)
    };

    TtsVoice {
        name: short,
        lang: v.locale.clone().unwrap_or_default(),
        display_name,
        gender,
    }
}

#[cfg(feature = "tts")]
#[tauri::command]
async fn get_tts_voices(locale: String) -> Vec<TtsVoice> {
    let ui_locale = locale.clone();
    let voices = match tokio::task::spawn_blocking(|| cached_voices().clone()).await {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let lang_prefix = if locale == "zh" { "zh" } else { "en" };

    let mut filtered: Vec<TtsVoice> = voices
        .iter()
        .filter(|v| {
            v.locale
                .as_deref()
                .map(|l| l.to_lowercase().starts_with(lang_prefix))
                .unwrap_or(false)
        })
        .map(|v| make_tts_voice(v, &ui_locale))
        .collect();

    if filtered.is_empty() {
        filtered = voices.iter().map(|v| make_tts_voice(v, &ui_locale)).collect();
    }

    filtered
}

#[cfg(feature = "tts")]
/// Synthesize text via Edge TTS and return raw MP3 bytes.
fn synthesize_tts(text: &str, voice: Option<&str>, locale: Option<&str>) -> Result<Vec<u8>, String> {
    let voices = cached_voices();

    let voice_name = match voice {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => {
            let lang_prefix = match locale {
                Some("zh") => "zh-CN",
                _ => "en-US",
            };
            voices
                .iter()
                .find(|v| {
                    v.locale
                        .as_deref()
                        .map(|l| l.starts_with(lang_prefix))
                        .unwrap_or(false)
                })
                .and_then(|v| v.short_name.clone())
                .unwrap_or_else(|| "en-US-AriaNeural".to_string())
        }
    };

    let speech_config = voices
        .iter()
        .find(|v| v.short_name.as_deref() == Some(&voice_name))
        .map(|v| msedge_tts::tts::SpeechConfig::from(v))
        .unwrap_or_else(|| msedge_tts::tts::SpeechConfig {
            voice_name: voice_name.clone(),
            audio_format: "audio-24khz-48kbitrate-mono-mp3".to_string(),
            pitch: 0,
            rate: 0,
            volume: 0,
        });

    log_debug(&format!("[tts] synthesizing with voice={voice_name}, text={:?}", truncate_for_log(text, 80)));

    let mut client =
        msedge_tts::tts::client::connect().map_err(|e| {
            let msg = format!("TTS connect error: {e}");
            log_debug(&format!("[tts] {msg}"));
            msg
        })?;
    let audio = client
        .synthesize(text, &speech_config)
        .map_err(|e| {
            let msg = format!("TTS synthesize error: {e}");
            log_debug(&format!("[tts] {msg}"));
            msg
        })?;

    log_debug(&format!("[tts] synthesized {} bytes of audio", audio.audio_bytes.len()));
    Ok(audio.audio_bytes)
}

#[cfg(feature = "tts")]
/// Play raw MP3 bytes through the system audio output using rodio.
fn play_mp3_bytes(bytes: &[u8]) -> Result<(), String> {
    use rodio::{Decoder, OutputStream, Sink};
    use std::io::Cursor;

    let (_stream, stream_handle) = OutputStream::try_default()
        .map_err(|e| {
            let msg = format!("audio output error: {e}");
            log_debug(&format!("[tts] {msg}"));
            msg
        })?;
    let source = Decoder::new(Cursor::new(bytes.to_vec()))
        .map_err(|e| {
            let msg = format!("MP3 decode error: {e}");
            log_debug(&format!("[tts] {msg}"));
            msg
        })?;
    let sink = Sink::try_new(&stream_handle)
        .map_err(|e| {
            let msg = format!("audio sink error: {e}");
            log_debug(&format!("[tts] {msg}"));
            msg
        })?;
    sink.append(source);
    sink.sleep_until_end();
    Ok(())
}

#[cfg(feature = "tts")]
/// Fallback TTS via macOS `say` command.
fn speak_with_say(text: &str, voice: Option<&str>, locale: Option<&str>) {
    log_debug(&format!("[tts] falling back to macOS say command"));
    let mut cmd = std::process::Command::new("say");
    if let Some(v) = voice.filter(|v| !v.is_empty()) {
        cmd.args(["--voice", v]);
    } else {
        let default_voice = match locale {
            Some("zh") => "Tingting",
            _ => "Samantha",
        };
        cmd.args(["--voice", default_voice]);
    }
    cmd.arg(text);
    match cmd.output() {
        Ok(o) if o.status.success() => log_debug("[tts] macOS say succeeded"),
        Ok(o) => log_debug(&format!("[tts] macOS say exited with status {}", o.status)),
        Err(e) => log_debug(&format!("[tts] macOS say failed: {e}")),
    }
}

#[cfg(feature = "tts")]
/// Global lock to serialize TTS playback — prevents overlapping audio when
/// multiple notifications arrive at the same time.
static TTS_PLAYBACK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "tts")]
/// Synthesize and play text, with automatic fallback to macOS `say`.
/// This is the core function used by both the Tauri command and backend notifications.
/// Acquires a global lock so that concurrent calls are queued, not overlapped.
pub(crate) fn speak_text_blocking(text: &str, voice: Option<&str>, locale: Option<&str>) {
    let _guard = TTS_PLAYBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match synthesize_tts(text, voice, locale) {
        Ok(bytes) => {
            if let Err(e) = play_mp3_bytes(&bytes) {
                log_debug(&format!("[tts] playback failed ({e}), falling back to say"));
                speak_with_say(text, voice, locale);
            }
        }
        Err(e) => {
            log_debug(&format!("[tts] Edge TTS failed ({e}), falling back to say"));
            speak_with_say(text, voice, locale);
        }
    }
}

#[cfg(feature = "tts")]
#[tauri::command]
async fn speak_text(
    text: String,
    voice: Option<String>,
    locale: Option<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        speak_text_blocking(&text, voice.as_deref(), locale.as_deref());
    })
    .await
    .map_err(|e| format!("TTS task failed: {e}"))
}

#[cfg(feature = "tts")]
#[tauri::command]
fn speak_text_say(text: String, voice: Option<String>, locale: Option<String>) {
    std::thread::spawn(move || {
        speak_with_say(&text, voice.as_deref(), locale.as_deref());
    });
}

#[cfg(feature = "tts")]
fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(feature = "tts")]
/// Read TTS settings from the Tauri store and play TTS for a notification summary.
/// Should be called from a background thread (blocks until playback finishes).
pub(crate) fn play_tts_for_notification(app: &tauri::AppHandle, summary: &str) {
    use tauri_plugin_store::StoreExt;

    let store = match app.store("settings.json") {
        Ok(s) => s,
        Err(e) => {
            log_debug(&format!("[tts] failed to open settings store: {e}"));
            return;
        }
    };

    let tts_mode = store.get("tts-mode")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "off".to_string());

    if tts_mode != "chime_and_speech" {
        return;
    }

    let muted = store.get("overlay-muted")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "false".to_string());

    if muted == "true" {
        log_debug("[tts] skipping notification TTS: overlay muted");
        return;
    }

    // Skip fallback/generic summaries
    const FALLBACK_SUMMARIES: &[&str] = &[
        "Status update", "Bug fixed", "Feature added", "Agent is stuck",
        "Agent ran into an issue", "Task completed", "Potential issues detected",
        "Agent is confused", "Task completed successfully", "Quick fix applied",
        "Extensive changes made", "Planning next steps", "Waiting for input",
    ];
    if FALLBACK_SUMMARIES.contains(&summary) {
        return;
    }

    let voice = store.get("tts-voice")
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    let locale = store.get("lang")
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    let locale_ref = locale.as_deref().map(|l| if l.starts_with("zh") { "zh" } else { "en" });

    log_debug(&format!("[tts] playing notification TTS for: {:?}", truncate_for_log(summary, 80)));
    speak_text_blocking(summary, voice.as_deref(), locale_ref);
}

#[cfg(not(feature = "tts"))]
#[tauri::command]
async fn get_tts_voices(_locale: String) -> Vec<TtsVoice> { vec![] }

#[cfg(not(feature = "tts"))]
#[tauri::command]
async fn speak_text(_text: String, _voice: Option<String>, _locale: Option<String>) -> Result<(), String> { Ok(()) }

#[cfg(not(feature = "tts"))]
#[tauri::command]
fn speak_text_say(_text: String, _voice: Option<String>, _locale: Option<String>) {}

#[cfg(not(feature = "tts"))]
pub(crate) fn play_tts_for_notification(_app: &tauri::AppHandle, _summary: &str) {}

// ── Notification mode ────────────────────────────────────────────────────────

#[tauri::command]
fn get_notification_mode(state: tauri::State<AppState>) -> String {
    state.notification_mode.lock().unwrap().clone()
}

#[tauri::command]
fn set_notification_mode(mode: String, state: tauri::State<AppState>) {
    let valid = matches!(mode.as_str(), "all" | "user_action" | "none");
    if valid {
        *state.notification_mode.lock().unwrap() = mode;
    }
}

#[tauri::command]
fn get_user_title(state: tauri::State<AppState>) -> String {
    state.user_title.lock().unwrap().clone()
}

#[tauri::command]
fn set_user_title(title: String, state: tauri::State<AppState>) {
    *state.user_title.lock().unwrap() = title;
}

#[tauri::command]
fn open_notification_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.notifications")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:notifications"])
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Best-effort for Linux / other — most DEs don't have a unified URL.
        let _ = std::process::Command::new("xdg-open")
            .arg("settings://notifications")
            .spawn();
    }
}

// ── Setup status check ───────────────────────────────────────────────────────

#[tauri::command]
async fn check_setup_status(state: tauri::State<'_, AppState>) -> Result<backend::SetupStatus, String> {
    // Only hold the backend lock briefly to get the cached session list,
    // then run the (potentially slow) subprocess checks outside the lock.
    let sessions = {
        let b = state.backend.read().unwrap();
        b.list_sessions()
    };
    let (cli_installed, cli_path) = check_cli_installed();
    let claude_dir_exists = dirs::home_dir()
        .map(|h| h.join(".claude").is_dir())
        .unwrap_or(false);
    let detected_tools = detect_installed_tools(&sessions);
    let logged_in = account::read_keychain_credentials().is_ok();
    let has_sessions = !sessions.is_empty();
    Ok(backend::SetupStatus {
        cli_installed,
        cli_path,
        claude_dir_exists,
        detected_tools,
        logged_in,
        has_sessions,
        credentials_valid: None,
    })
}

/// Detect which Claude-related tools are installed on the local machine.
/// Used by LocalBackend and fleet serve.
pub fn detect_installed_tools(sessions: &[SessionInfo]) -> backend::DetectedTools {
    let home = dirs::home_dir();

    // CLI: already checked via PATH / common paths
    let (cli, _) = check_cli_installed();

    // VS Code extension: check ~/.vscode/extensions/ and ~/.vscode-insiders/extensions/
    // (excludes ~/.cursor — that's tracked separately)
    let vscode = home.as_ref().map_or(false, |h| {
        let ext_dirs = [
            h.join(".vscode").join("extensions"),
            h.join(".vscode-insiders").join("extensions"),
        ];
        ext_dirs.iter().any(|dir| {
            dir.is_dir() && fs::read_dir(dir).map_or(false, |entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name().to_string_lossy().starts_with("anthropic.claude-code")
                })
            })
        })
    }) || sessions.iter().any(|s| {
        s.ide_name.as_deref().map_or(false, |name| {
            let n = name.to_lowercase();
            n.contains("vscode") || n.contains("vs code")
        })
    });

    // Cursor IDE: check if ~/.cursor exists
    let cursor = home.as_ref().map_or(false, |h| h.join(".cursor").is_dir());

    // OpenClaw: check if ~/.openclaw exists or `openclaw` is in PATH
    let openclaw = home.as_ref().map_or(false, |h| h.join(".openclaw").is_dir())
        || {
            #[cfg(unix)]
            { std::process::Command::new("which").arg("openclaw").output().map_or(false, |o| o.status.success()) }
            #[cfg(not(unix))]
            { std::process::Command::new("where").arg("openclaw").output().map_or(false, |o| o.status.success()) }
        };

    // JetBrains: check live sessions for JetBrains IDE names
    let jetbrains = sessions.iter().any(|s| {
        s.ide_name.as_deref().map_or(false, |name| {
            let n = name.to_lowercase();
            n.contains("intellij") || n.contains("webstorm") || n.contains("pycharm")
                || n.contains("goland") || n.contains("rustrover") || n.contains("phpstorm")
                || n.contains("rider") || n.contains("clion") || n.contains("jetbrains")
        })
    });

    // Claude Desktop app
    let desktop = {
        #[cfg(target_os = "macos")]
        { std::path::Path::new("/Applications/Claude.app").exists() }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA").map_or(false, |appdata| {
                std::path::Path::new(&appdata).join("Programs").join("Claude").join("Claude.exe").exists()
            })
        }
        #[cfg(target_os = "linux")]
        { false }
    };

    // Codex: check if ~/.codex exists or `codex` is in PATH
    let codex = home.as_ref().map_or(false, |h| h.join(".codex").is_dir())
        || {
            #[cfg(unix)]
            { std::process::Command::new("which").arg("codex").output().map_or(false, |o| o.status.success()) }
            #[cfg(not(unix))]
            { std::process::Command::new("where").arg("codex").output().map_or(false, |o| o.status.success()) }
        };

    // Apply source enable/disable config — disabled sources should not appear in the UI.
    let config = agent_source::SourcesConfig::load();
    let claude_enabled = config.is_source_enabled("claude");
    let cli = cli && claude_enabled;
    let vscode = vscode && claude_enabled;
    let jetbrains = jetbrains && claude_enabled;
    let desktop = desktop && claude_enabled;
    let cursor = cursor && config.is_source_enabled("cursor");
    let openclaw = openclaw && config.is_source_enabled("openclaw");
    let codex = codex && config.is_source_enabled("codex");

    backend::DetectedTools { cli, vscode, jetbrains, desktop, cursor, openclaw, codex }
}

pub fn check_cli_installed() -> (bool, Option<String>) {
    // Try `which claude` (unix) or `where claude` (windows)
    #[cfg(unix)]
    let cmd = "which";
    #[cfg(not(unix))]
    let cmd = "where";

    if let Ok(output) = std::process::Command::new(cmd).arg("claude").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return (true, Some(path));
        }
    }

    // Also check common install locations
    let common_paths = [
        dirs::home_dir().map(|h| h.join(".npm-global").join("bin").join("claude")),
        dirs::home_dir().map(|h| h.join(".local").join("bin").join("claude")),
        Some(std::path::PathBuf::from("/usr/local/bin/claude")),
        Some(std::path::PathBuf::from("/opt/homebrew/bin/claude")),
    ];

    for path_opt in &common_paths {
        if let Some(path) = path_opt {
            if path.exists() {
                return (true, Some(path.to_string_lossy().to_string()));
            }
        }
    }

    (false, None)
}

#[tauri::command]
async fn get_account_info(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<AccountInfo, String> {
    log_debug("get_account_info: start");
    let fut = state.backend.read().unwrap().account_info();
    let info = fut.await.map_err(|e| {
        log_debug(&format!("get_account_info: error: {e}"));
        e
    })?;
    // Update the Claude entry in cached usage summaries for the tray menu.
    {
        let app_state = app.state::<AppState>();
        let mut cached = app_state.cached_usage.lock().unwrap();
        let summary = backend::SourceUsageSummary::from_claude(&info);
        if let Some(pos) = cached.iter().position(|s| s.source == "claude") {
            cached[pos] = summary;
        } else {
            cached.insert(0, summary);
        }
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
    Ok(info)
}

#[tauri::command]
async fn get_source_account(
    source: String,
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    let fut = state.backend.read().unwrap().source_account(&source);
    fut.await
}

#[tauri::command]
async fn get_source_usage(
    source: String,
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Value, String> {
    let fut = state.backend.read().unwrap().source_usage(&source);
    let val = fut.await?;
    // Update the cached usage summary for the tray menu so that the
    // background refresh thread is no longer needed.
    {
        let summary = match source.as_str() {
            "cursor" => Some(backend::SourceUsageSummary::from_cursor(&val)),
            "codex" => Some(backend::SourceUsageSummary::from_codex(&val)),
            "openclaw" => Some(backend::SourceUsageSummary::from_openclaw(&val)),
            _ => None,
        };
        if let Some(summary) = summary {
            let app_state = app.state::<AppState>();
            let mut cached = app_state.cached_usage.lock().unwrap();
            if let Some(pos) = cached.iter().position(|s| s.source == source) {
                cached[pos] = summary;
            } else {
                cached.push(summary);
            }
        }
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
    Ok(val)
}

// ── Process kill ──────────────────────────────────────────────────────────────

#[tauri::command]
fn kill_session(pid: u32, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.read().unwrap().kill_pid(pid)
}

#[tauri::command]
fn kill_workspace_sessions(workspace_path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.read().unwrap().kill_workspace(workspace_path)
}

// ── App state ────────────────────────────────────────────────────────────────

pub struct AppState {
    /// The active backend (local or remote).  Swapped on connect/disconnect.
    /// Uses RwLock so read-only operations don't block each other (all Backend
    /// trait methods take &self).  Only the connect/disconnect swap needs a
    /// write lock.
    pub backend: Arc<RwLock<Box<dyn Backend>>>,
    /// User's current UI locale (e.g. "en", "zh"), shared with backend threads.
    pub locale: Arc<Mutex<String>>,
    /// Notification mode: "all" | "user_action" | "none".
    pub notification_mode: Arc<Mutex<String>>,
    /// How the assistant addresses the user (default "老板" / "Boss").
    pub user_title: Arc<Mutex<String>>,
    /// Cached sessions for tray menu rebuilds.
    pub cached_sessions: Arc<Mutex<Vec<SessionInfo>>>,
    /// Cached per-source usage summaries for tray menu display.
    pub cached_usage: Arc<Mutex<Vec<backend::SourceUsageSummary>>>,
    /// Fingerprint of the last tray menu content — skip rebuilds when unchanged
    /// to prevent the menu from closing while the user is interacting with it.
    pub tray_fingerprint: Arc<Mutex<u64>>,
    /// Timestamp of the last tray icon click.  While the menu is presumed open
    /// (within [`TRAY_MENU_GRACE_SECS`] of a click) we defer `set_menu` calls
    /// so macOS doesn't close the menu under the user's cursor.
    pub tray_last_click: Arc<Mutex<std::time::Instant>>,
    /// Whether a deferred tray rebuild is pending.
    pub tray_rebuild_pending: Arc<Mutex<bool>>,
    /// Timestamp when the tray panel was last shown.  Used to ignore the
    /// spurious focus-lost event that macOS fires when a tray click finishes.
    pub tray_panel_shown_at: Arc<Mutex<std::time::Instant>>,
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn list_sessions(state: tauri::State<AppState>) -> Vec<SessionInfo> {
    state.backend.read().unwrap().list_sessions()
}

#[tauri::command]
fn search_sessions(
    query: String,
    limit: Option<usize>,
    state: tauri::State<AppState>,
) -> Vec<search_index::SearchHit> {
    let limit = limit.unwrap_or(50);
    if query.trim().is_empty() {
        return vec![];
    }
    state.backend.read().unwrap().search_sessions(&query, limit)
}

#[tauri::command]
fn get_messages(
    jsonl_path: String,
    state: tauri::State<AppState>,
) -> Result<Vec<Value>, String> {
    state.backend.read().unwrap().get_messages(&jsonl_path)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SkillInvocation {
    skill: String,
    args: Option<String>,
    timestamp: String,
}

#[tauri::command]
fn get_skill_history(jsonl_path: String, state: tauri::State<AppState>) -> Result<Vec<SkillInvocation>, String> {
    let messages = state.backend.read().unwrap().get_messages(&jsonl_path)?;
    Ok(extract_skill_history(&messages))
}

fn extract_skill_history(messages: &[Value]) -> Vec<SkillInvocation> {
    let mut history = Vec::new();
    for msg in messages {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let timestamp = msg
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let Some(content_blocks) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content_blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && block.get("name").and_then(|n| n.as_str()) == Some("Skill")
            {
                if let Some(skill) = block
                    .get("input")
                    .and_then(|i| i.get("skill"))
                    .and_then(|s| s.as_str())
                {
                    let args = block
                        .get("input")
                        .and_then(|i| i.get("args"))
                        .and_then(|a| a.as_str())
                        .map(|s| s.to_string());
                    history.push(SkillInvocation {
                        skill: skill.to_string(),
                        args,
                        timestamp: timestamp.clone(),
                    });
                }
            }
        }
    }
    history
}

// ── Security audit ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_audit_events(state: tauri::State<AppState>) -> audit::AuditSummary {
    state.backend.read().unwrap().get_audit_events()
}

#[tauri::command]
fn get_daily_report(
    date: String,
    state: tauri::State<AppState>,
) -> Result<Option<daily_report::DailyReport>, String> {
    state.backend.read().unwrap().get_daily_report(&date)
}

#[tauri::command]
fn list_daily_report_stats(
    from: String,
    to: String,
    state: tauri::State<AppState>,
) -> Vec<daily_report::DailyReportStats> {
    state.backend.read().unwrap().list_daily_report_stats(&from, &to)
}

#[tauri::command]
async fn generate_daily_report(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<daily_report::DailyReport, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().generate_daily_report(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
async fn generate_daily_report_ai_summary(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().generate_daily_report_ai_summary(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
async fn generate_daily_report_lessons(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::daily_report::Lesson>, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().generate_daily_report_lessons(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
async fn append_lesson_to_claude_md(
    lesson: crate::daily_report::Lesson,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().append_lesson_to_claude_md(&lesson)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
fn check_pattern_update() -> String {
    pattern_update::check_update_now()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PatternInfo {
    version: u32,
    path: String,
}

#[tauri::command]
fn get_pattern_info() -> PatternInfo {
    let (version, path) = pattern_update::get_patterns_info();
    PatternInfo { version, path }
}

#[tauri::command]
fn start_watching_session(
    jsonl_path: String,
    state: tauri::State<AppState>,
) -> Result<u64, String> {
    state.backend.read().unwrap().start_watch(jsonl_path)
}

#[tauri::command]
fn stop_watching_session(state: tauri::State<AppState>) {
    state.backend.read().unwrap().stop_watch();
}

// ── Hooks setup ──────────────────────────────────────────────────────────────

#[tauri::command]
fn get_hooks_setup_plan(state: tauri::State<AppState>) -> hooks::HookSetupPlan {
    state.backend.read().unwrap().get_hooks_plan()
}

#[tauri::command]
fn apply_hooks_setup(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().apply_hooks()
}

#[tauri::command]
fn remove_hooks(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_hooks()
}

// ── CLI installer (macOS only) ───────────────────────────────────────────────

/// Create a symlink at /usr/local/bin/fleet pointing to the bundled fleet binary.
/// Requires the user to approve via osascript (admin password prompt).
#[tauri::command]
fn install_fleet_cli(app: tauri::AppHandle) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        // Tauri places externalBin sidecars next to the main executable
        let exe_dir = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("no parent dir")?
            .to_path_buf();
        let fleet_bin = exe_dir.join("fleet");
        if !fleet_bin.exists() {
            return Err(format!("fleet binary not found at {}", fleet_bin.display()));
        }

        let target = "/usr/local/bin/fleet";
        let src = fleet_bin.to_string_lossy().to_string();

        // Use osascript to run with admin privileges
        let script = format!(
            r#"do shell script "mkdir -p /usr/local/bin && ln -sf '{}' '{}'" with administrator privileges"#,
            src, target
        );
        let status = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status()
            .map_err(|e| e.to_string())?;

        if status.success() {
            Ok(target.to_string())
        } else {
            Err("Installation cancelled or failed".to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("install_fleet_cli is only supported on macOS".to_string())
    }
}

// ── Skill installer ──────────────────────────────────────────────────────────

pub const FLEET_SKILL_MD: &str = include_str!("../../skills/fleet/SKILL.md");

/// Tools we know support the Agent Skills standard, keyed by their home dir name.
pub const SKILL_TARGETS: &[(&str, &str)] = &[
    ("Claude Code", ".claude"),
    ("GitHub Copilot", ".copilot"),
    ("Cursor", ".cursor"),
    ("Gemini CLI", ".gemini"),
    ("OpenClaw", ".openclaw"),
];

#[derive(Serialize, Clone)]
struct DetectedTool {
    name: String,
    skill_path: String,
}

#[derive(Serialize)]
struct SkillInstallResult {
    installed: Vec<DetectedTool>,
    errors: Vec<String>,
}

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .map_err(|_| "Cannot determine home directory".to_string())
}

/// Detect which AI tools are installed (by checking their home directories).
#[tauri::command]
fn detect_ai_tools() -> Result<Vec<DetectedTool>, String> {
    let home = home_dir()?;
    let detected = SKILL_TARGETS
        .iter()
        .filter(|(_, dir)| home.join(dir).exists())
        .map(|(name, dir)| DetectedTool {
            name: name.to_string(),
            skill_path: home
                .join(dir)
                .join("skills")
                .join("fleet")
                .join("SKILL.md")
                .to_string_lossy()
                .to_string(),
        })
        .collect();
    Ok(detected)
}

/// Open a native file-open dialog and return the chosen path.
#[tauri::command]
async fn pick_file(title: String) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title(&title)
        .pick_file()
        .await
        .map(|f| f.path().to_string_lossy().to_string())
}

/// Open a native save dialog and write SKILL.md to the chosen path.
#[tauri::command]
async fn save_skill_file() -> Result<String, String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_file_name("SKILL.md")
        .set_title("Save Fleet Skill")
        .save_file()
        .await;

    match handle {
        Some(file) => {
            file.write(FLEET_SKILL_MD.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            Ok(file.path().to_string_lossy().to_string())
        }
        None => Err("cancelled".to_string()),
    }
}

/// Install the fleet skill to all detected AI tool directories.
#[tauri::command]
fn install_fleet_skill() -> Result<SkillInstallResult, String> {
    let home = home_dir()?;
    let mut installed = vec![];
    let mut errors = vec![];

    for (name, dir) in SKILL_TARGETS {
        let tool_home = home.join(dir);
        if !tool_home.exists() {
            continue;
        }
        let skill_dir = tool_home.join("skills").join("fleet");
        let skill_path = skill_dir.join("SKILL.md");
        match std::fs::create_dir_all(&skill_dir)
            .and_then(|_| std::fs::write(&skill_path, FLEET_SKILL_MD))
        {
            Ok(_) => installed.push(DetectedTool {
                name: name.to_string(),
                skill_path: skill_path.to_string_lossy().to_string(),
            }),
            Err(e) => errors.push(format!("{}: {}", name, e)),
        }
    }

    if installed.is_empty() && errors.is_empty() {
        errors.push(
            "No supported AI tools detected. Install Claude Code, Cursor, GitHub Copilot, or Gemini CLI first.".to_string(),
        );
    }

    Ok(SkillInstallResult { installed, errors })
}

// ── TCC diagnostics ─────────────────────────────────────────────────────────

/// Run macOS TCC diagnostics to identify what triggers permission dialogs.
#[tauri::command]
fn diagnose_tcc() -> tcc::TccDiagnostic {
    let diag = tcc::diagnose();
    // Also write findings to the debug log.
    if diag.findings.is_empty() {
        log_debug("[TCC-DIAG] No TCC-triggering code paths detected.");
    } else {
        log_debug(&format!(
            "[TCC-DIAG] Found {} potential TCC triggers:",
            diag.findings.len()
        ));
        for f in &diag.findings {
            log_debug(&format!(
                "[TCC-DIAG]   [{}/{}] {} — {}",
                f.component, f.tcc_category, f.path, f.reason,
            ));
        }
    }
    log_debug(&format!(
        "[TCC-DIAG] sysinfo: {} processes scanned, {} matched, phase1_safe={}",
        diag.sysinfo_process_count,
        diag.sysinfo_matched_processes.len(),
        diag.sysinfo_phase1_safe,
    ));
    if !diag.tcc_triggering_workspaces.is_empty() {
        log_debug(&format!(
            "[TCC-DIAG] {} workspace dirs would trigger TCC during decode:",
            diag.tcc_triggering_workspaces.len(),
        ));
        for ws in &diag.tcc_triggering_workspaces {
            for tp in &ws.tcc_paths {
                log_debug(&format!(
                    "[TCC-DIAG]   '{}' → probes {} ({})",
                    ws.encoded, tp.path, tp.tcc_category,
                ));
            }
        }
    }
    diag
}

// ── Memory commands ──────────────────────────────────────────────────────────

#[tauri::command]
fn list_memories(state: tauri::State<AppState>) -> Vec<memory::WorkspaceMemory> {
    state.backend.read().unwrap().list_memories()
}

#[tauri::command]
fn get_memory_content(path: String, state: tauri::State<AppState>) -> Result<String, String> {
    state.backend.read().unwrap().get_memory_content(&path)
}

#[tauri::command]
fn get_memory_history(path: String, state: tauri::State<AppState>) -> Vec<memory::MemoryHistoryEntry> {
    state.backend.read().unwrap().get_memory_history(&path)
}

#[tauri::command]
fn get_claude_md_content(workspace_path: String) -> Result<String, String> {
    memory::read_claude_md(&workspace_path)
}

#[tauri::command]
fn promote_memory(memory_path: String, target: String, workspace_path: String) -> Result<(), String> {
    memory::promote_memory(&memory_path, &target, &workspace_path)
}

// ── Skills ────────────────────────────────────────────────────────────────────

#[tauri::command]
fn list_skills(state: tauri::State<AppState>) -> Vec<skills::SkillItem> {
    state.backend.read().unwrap().list_skills()
}

#[tauri::command]
fn get_skill_content(path: String, state: tauri::State<AppState>) -> Result<String, String> {
    state.backend.read().unwrap().get_skill_content(&path)
}

// ── Agent sources config ─────────────────────────────────────────────────────

/// Return the current sources config merged with availability info.
#[tauri::command]
fn get_sources_config(state: tauri::State<AppState>) -> Vec<agent_source::SourceInfo> {
    state.backend.read().unwrap().get_sources_config()
}

/// Toggle a source on/off and persist to disk (local or remote).
#[tauri::command]
fn set_source_enabled(name: String, enabled: bool, state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().set_source_enabled(&name, enabled)
}

// ── App restart ─────────────────────────────────────────────────────────────

#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

// ── Locale ──────────────────────────────────────────────────────────────────

#[tauri::command]
fn set_locale(locale: String, state: tauri::State<AppState>) {
    *state.locale.lock().unwrap() = locale;
}

// ── Waiting alerts ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_waiting_alerts(state: tauri::State<AppState>) -> Vec<backend::WaitingAlert> {
    state.backend.read().unwrap().get_waiting_alerts()
}

// ── Mascot quip generation ──────────────────────────────────────────────────

#[tauri::command]
async fn generate_mascot_quips(busy_titles: Vec<String>, done_titles: Vec<String>, locale: String) -> claude_analyze::MascotQuips {
    tokio::task::spawn_blocking(move || {
        claude_analyze::generate_mascot_quips(&busy_titles, &done_titles, &locale)
    })
    .await
    .unwrap_or_default()
}

// ── Overlay window commands ──────────────────────────────────────────────────

#[tauri::command]
fn toggle_overlay(app: tauri::AppHandle, visible: bool) {
    if let Some(w) = app.get_webview_window("overlay") {
        if visible {
            // Move on-screen (bottom-right). Using position instead of
            // show/hide avoids the transparent-window white-flash bug on macOS.
            let _ = w.show();
            if let Ok(Some(monitor)) = w.current_monitor() {
                let size = monitor.size();
                let scale = monitor.scale_factor();
                let x = (size.width as f64 / scale) as i32 - 300;
                let y = (size.height as f64 / scale) as i32 - 220;
                let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                    x as f64, y as f64,
                )));
            } else {
                let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                    100.0, 100.0,
                )));
            }
        } else {
            // Move off-screen to "hide"
            let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(
                -9999.0, -9999.0,
            )));
        }
    }
}

#[tauri::command]
fn center_overlay(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        if let Ok(Some(monitor)) = w.current_monitor() {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            if let Ok(win_size) = w.outer_size() {
                let x = (size.width as f64 / scale - win_size.width as f64 / scale) / 2.0;
                let y = (size.height as f64 / scale - win_size.height as f64 / scale) / 2.0;
                let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
            }
        }
    }
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

#[tauri::command]
fn open_session_from_overlay(app: tauri::AppHandle, jsonl_path: String) {
    let _ = app.emit("open-session", jsonl_path);
}

// ── Tray panel window commands ─────────────────────────────────────────────

/// Show the tray panel at the given physical click position.
fn show_tray_panel_at(app: &tauri::AppHandle, click_x: f64, click_y: f64) {
    let Some(w) = app.get_webview_window("tray_panel") else { return };

    // Find the monitor that contains the click point so we use the
    // correct scale factor (each display can have a different DPI).
    let scale = app.available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|m| {
                let pos = m.position();
                let size = m.size();
                let (mx, my) = (pos.x as f64, pos.y as f64);
                let (mw, mh) = (size.width as f64, size.height as f64);
                click_x >= mx && click_x < mx + mw
                    && click_y >= my && click_y < my + mh
            })
        })
        .map(|m| m.scale_factor())
        .unwrap_or(2.0);

    // The click position is in physical pixels (global screen coords).
    // Convert panel width to physical pixels for centering.
    let panel_w_phys = 360.0 * scale;
    let x = click_x - panel_w_phys / 2.0;

    // macOS: menu bar at top → panel below click.
    // Windows/Linux: taskbar usually at bottom → panel above click.
    #[cfg(target_os = "macos")]
    let y = click_y;
    #[cfg(not(target_os = "macos"))]
    let y = click_y - 520.0 * scale;

    let _ = w.set_position(tauri::Position::Physical(
        tauri::PhysicalPosition::new(x as i32, y as i32),
    ));
    // Record show timestamp *before* showing, so the window-event
    // listener can see it immediately — no IPC delay.
    let state = app.state::<AppState>();
    *state.tray_panel_shown_at.lock().unwrap() = std::time::Instant::now();

    let _ = w.show();
    let _ = w.set_focus();

    // Emit cached usage summaries so the panel can show them immediately.
    let summaries = state.cached_usage.lock().unwrap().clone();
    let _ = app.emit_to("tray_panel", "tray-usage-updated", &summaries);
}

fn hide_tray_panel(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("tray_panel") {
        let _ = w.set_position(tauri::Position::Logical(
            tauri::LogicalPosition::new(-9999.0, -9999.0),
        ));
    }
}

fn is_tray_panel_visible(app: &tauri::AppHandle) -> bool {
    app.get_webview_window("tray_panel")
        .and_then(|w| w.outer_position().ok())
        .map(|pos| pos.x > -5000 && pos.y > -5000)
        .unwrap_or(false)
}

#[tauri::command]
fn toggle_tray_panel(app: tauri::AppHandle, visible: bool) {
    if visible {
        show_tray_panel_at(&app, 500.0, 30.0);
    } else {
        hide_tray_panel(&app);
    }
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

// ── Tray helpers ─────────────────────────────────────────────────────────────

fn status_label(s: &session::SessionStatus) -> &'static str {
    use session::SessionStatus::*;
    match s {
        Thinking => "thinking",
        Executing => "executing",
        Streaming => "streaming",
        Processing => "processing",
        WaitingInput => "waiting input",
        Active => "active",
        Delegating => "delegating",
        Idle => "idle",
    }
}

fn is_session_active(s: &SessionInfo) -> bool {
    use session::SessionStatus;
    matches!(
        s.status,
        SessionStatus::Thinking | SessionStatus::Executing |
        SessionStatus::Streaming | SessionStatus::Processing |
        SessionStatus::WaitingInput | SessionStatus::Active |
        SessionStatus::Delegating
    )
}

pub fn update_tray(app: &tauri::AppHandle, sessions: &[SessionInfo]) {
    // Cache sessions for use by background usage refresh.
    let state = app.state::<AppState>();
    *state.cached_sessions.lock().unwrap() = sessions.to_vec();
    // Tray operations (set_menu, set_tooltip, set_title) touch NSStatusItem on
    // macOS and MUST run on the main thread.  This function is often called
    // from background scanner threads, so dispatch rather than calling directly.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
}

pub fn update_tray_usage(app: &tauri::AppHandle, summaries: Vec<backend::SourceUsageSummary>) {
    // Also emit to the tray panel window so it can show live usage
    let _ = app.emit_to("tray_panel", "tray-usage-updated", &summaries);
    let state = app.state::<AppState>();
    *state.cached_usage.lock().unwrap() = summaries;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
}

/// How long after a tray click we assume the menu is still open and defer
/// rebuilds so macOS doesn't yank it away from the user.
const TRAY_MENU_GRACE_SECS: u64 = 15;

fn rebuild_tray(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let sessions = state.cached_sessions.lock().unwrap().clone();
    let summaries = state.cached_usage.lock().unwrap().clone();

    // Show all active sessions (main + subagents), sorted: main first, then subs.
    let mut active_all: Vec<&SessionInfo> = sessions.iter()
        .filter(|s| is_session_active(s))
        .collect();
    active_all.sort_by_key(|s| s.is_subagent);
    let active_main = &active_all; // alias for build_tray_menu signature
    let sub_count = active_all.iter().filter(|s| s.is_subagent).count();
    let total = active_all.len();

    // Compute a fingerprint of the tray content so we can skip redundant
    // menu rebuilds — calling set_menu() closes the menu if it is open.
    let fingerprint = {
        let mut h = DefaultHasher::new();
        total.hash(&mut h);
        sub_count.hash(&mut h);
        for s in active_main.iter() {
            s.workspace_name.hash(&mut h);
            s.is_subagent.hash(&mut h);
            status_label(&s.status).hash(&mut h);
        }
        for su in &summaries {
            su.source.hash(&mut h);
            for b in &su.bars {
                b.label.hash(&mut h);
                ((b.utilization * 10000.0) as u64).hash(&mut h);
            }
        }
        h.finish()
    };

    let prev = {
        let mut fp = state.tray_fingerprint.lock().unwrap();
        let old = *fp;
        *fp = fingerprint;
        old
    };

    // Update tooltip & title (cheap, won't close menu)
    let tooltip = if total == 0 {
        "Claw Fleet".to_string()
    } else {
        format!(
            "Claw Fleet — {} active  (Main: {}  Sub: {})",
            total, active_main.len(), sub_count
        )
    };

    let Some(tray) = app.tray_by_id("main") else { return };
    let _ = tray.set_tooltip(Some(&tooltip));
    #[cfg(target_os = "macos")]
    {
        let title = if total > 0 { format!("{}", total) } else { String::new() };
        let _ = tray.set_title(Some(&title));
    }

    // Only rebuild the menu when content actually changed.
    if fingerprint != prev {
        // If the menu is presumed open (recent tray click), defer the rebuild
        // so we don't close it under the user's cursor.
        let since_click = state.tray_last_click.lock().unwrap().elapsed();
        if since_click < std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS) {
            *state.tray_rebuild_pending.lock().unwrap() = true;
            return;
        }

        if let Ok(menu) = build_tray_menu(app, active_main, sub_count, total, &summaries) {
            let _ = tray.set_menu(Some(menu));
        }
        *state.tray_rebuild_pending.lock().unwrap() = false;
    }
}

/// Flush any deferred tray rebuild.  Called from a background timer once the
/// grace period after a tray click has expired.
fn flush_pending_tray_rebuild(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let pending = *state.tray_rebuild_pending.lock().unwrap();
    if !pending { return; }

    let since_click = state.tray_last_click.lock().unwrap().elapsed();
    if since_click < std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS) {
        return; // still within grace period
    }

    // Force a rebuild by resetting the fingerprint so the next call rebuilds.
    *state.tray_fingerprint.lock().unwrap() = 0;
    *state.tray_rebuild_pending.lock().unwrap() = false;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
}

/// Render a utilization value (0.0–1.0) as a percentage string, e.g. `45%`.
fn usage_pct_str(utilization: f64) -> String {
    let pct = (utilization * 100.0).round() as u32;
    format!("{}%", pct)
}

fn build_tray_menu(
    app: &tauri::AppHandle,
    active_main: &[&SessionInfo],
    _sub_count: usize,
    total: usize,
    summaries: &[backend::SourceUsageSummary],
) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    let mut builder = MenuBuilder::new(app);

    // ── Active agents section ────────────────────────────────────────────
    let header_text = if total > 0 {
        format!("{} Active Agent{}", total, if total == 1 { "" } else { "s" })
    } else {
        "No Active Agents".to_string()
    };
    builder = builder.item(
        &MenuItemBuilder::new(header_text).id("info-header").enabled(false).build(app)?
    );

    // List all active sessions (main + subagents), clickable to open detail.
    for (i, s) in active_main.iter().enumerate() {
        let prefix = if s.is_subagent { "  ↳ " } else { "" };
        let label = format!("{}{} — {}", prefix, s.workspace_name, status_label(&s.status));
        builder = builder.item(
            &MenuItemBuilder::new(label).id(format!("open-session-{}", i)).build(app)?
        );
    }

    builder = builder.item(&PredefinedMenuItem::separator(app)?);

    // ── Usage section (all sources) ─────────────────────────────────────
    if !summaries.is_empty() {
        for (idx, summary) in summaries.iter().enumerate() {
            if summary.bars.is_empty() {
                continue;
            }
            let parts: Vec<String> = summary.bars.iter()
                .map(|b| format!("{}\t{}", b.label, usage_pct_str(b.utilization)))
                .collect();
            let source_label = match summary.source.as_str() {
                "claude" => "Claude",
                "cursor" => "Cursor",
                "codex" => "Codex",
                "openclaw" => "OpenClaw",
                other => other,
            };
            let line = format!("{}\t{}", source_label, parts.join("\t"));
            builder = builder.item(
                &MenuItemBuilder::new(line)
                    .id(format!("info-usage-{}", idx))
                    .enabled(true)
                    .build(app)?
            );
        }
        builder = builder.item(&PredefinedMenuItem::separator(app)?);
    }

    // ── Actions ──────────────────────────────────────────────────────────
    builder = builder.item(
        &MenuItemBuilder::new("Switch Connection").id("switch-connection").build(app)?
    );
    builder = builder.item(&PredefinedMenuItem::separator(app)?);
    builder = builder.item(
        &MenuItemBuilder::new("Quit").id("quit").build(app)?
    );

    builder.build()
}

// ── App setup ────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState {
            // NullBackend is a placeholder; replaced with LocalBackend in setup().
            backend: Arc::new(RwLock::new(Box::new(backend::NullBackend) as Box<dyn Backend>)),
            locale: Arc::new(Mutex::new("en".to_string())),
            notification_mode: Arc::new(Mutex::new("user_action".to_string())),
            user_title: Arc::new(Mutex::new(String::new())),
            cached_sessions: Arc::new(Mutex::new(Vec::new())),
            cached_usage: Arc::new(Mutex::new(Vec::new())),
            tray_fingerprint: Arc::new(Mutex::new(0)),
            tray_last_click: Arc::new(Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(600))),
            tray_rebuild_pending: Arc::new(Mutex::new(false)),
            tray_panel_shown_at: Arc::new(Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(600))),
        })
        .setup(move |app| {
            log_debug("[TCC-SETUP] App setup starting — querying TCC before backend init");
            tcc::log_recent_tcc_events();

            // Replace NullBackend with the real LocalBackend now that AppHandle
            // is available.
            {
                let state = app.state::<AppState>();
                let locale = state.locale.clone();

                // Build the agent source registry from config (~/.claude/fleet-sources.json).
                let sources = agent_source::build_sources();

                let local = local_backend::LocalBackend::new(
                    app.handle().clone(),
                    locale,
                    sources,
                );
                *state.backend.write().unwrap() = Box::new(local);
            }

            log_debug("[TCC-SETUP] Backend initialized — querying TCC after backend init");
            tcc::log_recent_tcc_events();

            // Truncate the hook events file if it has grown too large.
            hooks::maybe_truncate_events_file();

            // ── Audit pattern updates ───────────────────────────────────────
            // Seed local patterns from bundled resource (first run or app
            // upgrade), then start the daily background updater.
            pattern_update::bootstrap_patterns(app.handle());
            pattern_update::start_background_updater();

            // Background usage refresh removed — the frontend's periodic
            // `get_source_usage` / `get_account_info` calls now update the
            // cached tray summaries as a side-effect, avoiding duplicate
            // network requests that could hit rate limits.

            // ── Tray icon ────────────────────────────────────────────────────
            // Build an initial menu; it will be rebuilt dynamically by rebuild_tray().
            let tray_menu = MenuBuilder::new(app)
                .item(&MenuItemBuilder::new("No Active Agents").id("info-header").enabled(false).build(app)?)
                .item(&PredefinedMenuItem::separator(app)?)
                .item(&MenuItemBuilder::new("Switch Connection").id("switch-connection").build(app)?)
                .item(&PredefinedMenuItem::separator(app)?)
                .item(&MenuItemBuilder::new("Quit").id("quit").build(app)?)
                .build()?;

            #[cfg(target_os = "macos")]
            let tray_builder = {
                let icon = load_png_as_tray_icon(include_bytes!("../icons/tray-macos.png"));
                TrayIconBuilder::with_id("main")
                    .icon(icon)
                    .icon_as_template(true)
            };

            #[cfg(target_os = "windows")]
            let tray_builder = {
                let icon = load_png_as_tray_icon(include_bytes!("../icons/tray-windows.png"));
                TrayIconBuilder::with_id("main")
                    .icon(icon)
            };

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_builder = {
                let icon = app.default_window_icon().cloned().unwrap();
                TrayIconBuilder::with_id("main")
                    .icon(icon)
            };

            tray_builder
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .tooltip("Claw Fleet")
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { position, button, button_state, .. } = &event {
                        if !matches!(button, tauri::tray::MouseButton::Left) {
                            return;
                        }
                        if !matches!(button_state, tauri::tray::MouseButtonState::Up) {
                            return;
                        }

                        let app = tray.app_handle();
                        let state = app.state::<AppState>();
                        *state.tray_last_click.lock().unwrap() = std::time::Instant::now();

                        if is_tray_panel_visible(app) {
                            hide_tray_panel(app);
                        } else {
                            show_tray_panel_at(app, position.x, position.y);
                        }
                    }
                })
                .on_menu_event(|app, event| {
                    let id = event.id();
                    let id_str = id.as_ref();
                    if id_str == "switch-connection" {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                        let _ = app.emit("switch-connection", ());
                    } else if id_str == "quit" {
                        app.exit(0);
                    } else if let Some(idx_str) = id_str.strip_prefix("open-session-") {
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            let state = app.state::<AppState>();
                            let sessions = state.cached_sessions.lock().unwrap().clone();
                            let mut active: Vec<&SessionInfo> = sessions.iter()
                                .filter(|s| is_session_active(s))
                                .collect();
                            active.sort_by_key(|s| s.is_subagent);
                            if let Some(s) = active.get(idx) {
                                // Show the main window and emit the session to open.
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                                let _ = app.emit("open-session", s.jsonl_path.clone());
                            }
                        }
                    }
                })
                .build(app)?;

            // Background thread to flush deferred tray rebuilds once the
            // grace period after a tray click has elapsed.
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS));
                        flush_pending_tray_rebuild(&app_handle);
                    }
                });
            }

            // ── Tray panel: auto-hide on focus loss ─────────────────────────
            // Handled entirely in Rust to avoid IPC timing issues.
            if let Some(panel) = app.get_webview_window("tray_panel") {
                let app_handle = app.handle().clone();
                panel.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        if cfg!(target_os = "macos") {
                            let state = app_handle.state::<AppState>();
                            let elapsed = state.tray_panel_shown_at.lock().unwrap().elapsed();
                            if elapsed < std::time::Duration::from_millis(600) {
                                // macOS steals focus when the status-bar click
                                // finishes.  Re-focus the panel after a short
                                // delay so that subsequent outside-clicks will
                                // trigger a proper blur → hide.
                                let handle = app_handle.clone();
                                std::thread::spawn(move || {
                                    std::thread::sleep(std::time::Duration::from_millis(200));
                                    if is_tray_panel_visible(&handle) {
                                        if let Some(w) = handle.get_webview_window("tray_panel") {
                                            let _ = w.set_focus();
                                        }
                                    }
                                });
                                return;
                            }
                        }
                        hide_tray_panel(&app_handle);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            search_sessions,
            get_messages,
            get_skill_history,
            get_audit_events,
            check_pattern_update,
            get_pattern_info,
            start_watching_session,
            stop_watching_session,
            get_account_info,
            get_log_path,
            get_platform,
            check_app_version,
            kill_session,
            kill_workspace_sessions,
            check_setup_status,
            install_fleet_cli,
            detect_ai_tools,
            install_fleet_skill,
            save_skill_file,
            remote::list_saved_connections,
            remote::list_ssh_profiles,
            remote::delete_connection,
            remote::connect_remote,
            remote::disconnect_remote,
            pick_file,
            diagnose_tcc,
            get_source_account,
            get_source_usage,
            list_memories,
            get_memory_content,
            get_memory_history,
            get_claude_md_content,
            promote_memory,
            list_skills,
            get_skill_content,
            get_waiting_alerts,
            set_locale,
            get_hooks_setup_plan,
            apply_hooks_setup,
            remove_hooks,
            generate_mascot_quips,
            get_sources_config,
            set_source_enabled,
            restart_app,
            get_notification_mode,
            set_notification_mode,
            get_user_title,
            set_user_title,
            open_notification_settings,
            toggle_overlay,
            center_overlay,
            show_main_window,
            open_session_from_overlay,
            toggle_tray_panel,
            quit_app,
            get_tts_voices,
            speak_text,
            speak_text_say,
            get_daily_report,
            list_daily_report_stats,
            generate_daily_report,
            generate_daily_report_ai_summary,
            generate_daily_report_lessons,
            append_lesson_to_claude_md,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
