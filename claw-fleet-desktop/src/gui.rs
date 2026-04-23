// GUI-specific code, only compiled with the "gui" feature.
//
// Extracted from lib.rs to avoid pulling tauri/image/rfd/notify into the
// fleet-cli probe binary.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};

use std::sync::OnceLock;

use serde::Serialize;
use serde_json::Value;
use tauri::{Emitter, Listener, Manager};
use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;

use super::account::AccountInfo;
use super::backend::Backend;
use super::session::SessionInfo;
use super::*;

fn load_png_as_tray_icon(bytes: &[u8]) -> tauri::image::Image<'static> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("failed to decode tray icon PNG")
        .into_rgba8();
    let (w, h) = img.dimensions();
    tauri::image::Image::new_owned(img.into_raw(), w, h)
}

#[tauri::command]
fn get_log_path() -> String {
    session::real_home_dir()
        .map(|h| h.join(".fleet").join("claw-fleet-debug.log").to_string_lossy().to_string())
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

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ── TTS via Microsoft Edge TTS ───────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
struct TtsVoice {
    name: String,
    lang: String,
    display_name: String,
    gender: String,
}


static VOICES_CACHE: std::sync::Mutex<Option<Vec<msedge_tts::voice::Voice>>> =
    std::sync::Mutex::new(None);


fn cached_voices() -> Vec<msedge_tts::voice::Voice> {
    {
        let guard = VOICES_CACHE.lock().unwrap();
        if let Some(ref v) = *guard {
            return v.clone();
        }
    }
    // Not cached yet — fetch (may fail on bad network)
    match msedge_tts::voice::get_voices_list() {
        Ok(voices) if !voices.is_empty() => {
            let mut guard = VOICES_CACHE.lock().unwrap();
            *guard = Some(voices.clone());
            voices
        }
        _ => vec![],
    }
}


struct VoiceMeta {
    zh_name: &'static str,
    en_name: &'static str,
    gender_zh: &'static str,
    gender_en: &'static str,
}


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


#[tauri::command]
async fn get_tts_voices(locale: String) -> Vec<TtsVoice> {
    let ui_locale = locale.clone();
    let voices = match tokio::task::spawn_blocking(cached_voices).await {
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


/// Global lock to serialize TTS playback — prevents overlapping audio when
/// multiple notifications arrive at the same time.
static TTS_PLAYBACK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());


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


#[tauri::command]
fn speak_text_say(text: String, voice: Option<String>, locale: Option<String>) {
    std::thread::spawn(move || {
        speak_with_say(&text, voice.as_deref(), locale.as_deref());
    });
}


fn truncate_for_log(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
}


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

    let muted = store.get("tts-muted")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "false".to_string());

    if muted == "true" {
        log_debug("[tts] skipping notification TTS: muted");
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
    *state.user_title.lock().unwrap() = title.clone();
    reapply_interaction_mode_if_installed(&state, &title, None);
}

/// If the interaction-mode guidance is currently installed, regenerate it with
/// fresh title/locale values. Silent on failure — it's a convenience re-sync.
fn reapply_interaction_mode_if_installed(
    state: &tauri::State<AppState>,
    title_override: &str,
    locale_override: Option<&str>,
) {
    let backend = state.backend.read().unwrap();
    let plan = backend.get_hooks_plan();
    if !plan.interaction_mode_installed {
        return;
    }
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = backend.apply_interaction_mode(title_override, &locale) {
        eprintln!("re-apply interaction mode failed: {e}");
    }
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
    let claude_dir_exists = session::real_home_dir()
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

#[tauri::command]
fn resume_rate_limited_session(
    session_id: String,
    workspace_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .resume_session(session_id, workspace_path)
}

#[tauri::command]
fn get_auto_resume_config(
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::auto_resume::AutoResumeConfig {
    state.backend.read().unwrap().get_auto_resume_config()
}

#[tauri::command]
fn set_auto_resume_config(
    config: claw_fleet_core::auto_resume::AutoResumeConfig,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().set_auto_resume_config(config)
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
    /// LLM provider config (which CLI + models to use for analysis/reports).
    pub llm_config: Arc<Mutex<llm_provider::LlmConfig>>,
    /// Cached LLM provider info — pre-fetched at startup so Settings opens instantly.
    pub cached_llm_providers: Arc<Mutex<Vec<llm_provider::LlmProviderInfo>>>,
    /// Mobile access: embedded HTTP server (None = not started).
    pub mobile_server: Arc<Mutex<Option<embedded_server::EmbeddedServer>>>,
    /// Mobile access: Cloudflare tunnel (None = not started).
    pub mobile_tunnel: Arc<Mutex<Option<tunnel::CloudflareTunnel>>>,
    /// Whether mobile access setup (download + tunnel) is in progress.
    pub mobile_setup_in_progress: Arc<std::sync::atomic::AtomicBool>,
    /// Serialized snapshot of the current decision queue, seeded by the main
    /// window before it pops the decision-float. The float window reads this
    /// on mount to hydrate its local store before live events arrive.
    pub decision_float_snapshot: Arc<Mutex<Option<serde_json::Value>>>,
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

#[tauri::command]
fn get_session_todos(
    jsonl_path: String,
    state: tauri::State<AppState>,
) -> Result<Vec<claw_fleet_core::session_todos::TodoItem>, String> {
    let messages = state.backend.read().unwrap().get_messages(&jsonl_path)?;
    Ok(claw_fleet_core::session_todos::extract_latest_todos(&messages))
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
fn get_audit_rules(state: tauri::State<AppState>) -> Vec<audit::AuditRuleInfo> {
    state.backend.read().unwrap().get_audit_rules()
}

#[tauri::command]
fn set_audit_rule_enabled(state: tauri::State<AppState>, id: String, enabled: bool) -> Result<(), String> {
    state.backend.read().unwrap().set_audit_rule_enabled(&id, enabled)
}

#[tauri::command]
fn save_custom_audit_rule(state: tauri::State<AppState>, rule: audit::AuditRuleInfo) -> Result<(), String> {
    state.backend.read().unwrap().save_custom_audit_rule(rule)
}

#[tauri::command]
fn delete_custom_audit_rule(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    state.backend.read().unwrap().delete_custom_audit_rule(&id)
}

#[tauri::command]
fn suggest_audit_rules(state: tauri::State<AppState>, concern: String, lang: String) -> Result<Vec<audit::SuggestedRule>, String> {
    state.backend.read().unwrap().suggest_audit_rules(&concern, &lang)
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

// ── Guard (real-time interception) ───────────────────────────────────────────

#[tauri::command]
fn apply_guard_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().apply_guard_hook()
}

#[tauri::command]
fn remove_guard_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_guard_hook()
}

#[tauri::command]
fn respond_to_guard(state: tauri::State<AppState>, id: String, allow: bool) -> Result<(), String> {
    state.backend.read().unwrap().respond_to_guard(&id, allow)
}

#[tauri::command]
async fn analyze_guard_command(
    state: tauri::State<'_, AppState>,
    command: String,
    context: String,
    lang: String,
) -> Result<String, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend
            .read()
            .unwrap()
            .analyze_guard_command(&command, &context, &lang)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

// ── Elicitation (AskUserQuestion interception) ──────────────────────────────

#[tauri::command]
fn apply_elicitation_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().apply_elicitation_hook()
}

#[tauri::command]
fn remove_elicitation_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_elicitation_hook()
}

#[tauri::command]
fn apply_interaction_mode(state: tauri::State<AppState>) -> Result<(), String> {
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    state.backend.read().unwrap().apply_interaction_mode(&title, &locale)
}

#[tauri::command]
fn remove_interaction_mode(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_interaction_mode()
}

#[tauri::command]
fn respond_to_elicitation(
    state: tauri::State<AppState>,
    id: String,
    declined: bool,
    answers: std::collections::HashMap<String, String>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .respond_to_elicitation(&id, declined, answers)
}

#[tauri::command]
fn upload_elicitation_attachment(
    state: tauri::State<AppState>,
    source_path: String,
) -> Result<String, String> {
    state
        .backend
        .read()
        .unwrap()
        .upload_attachment(std::path::Path::new(&source_path))
}

/// Writes clipboard/drag-drop bytes to the OS temp dir and returns the absolute
/// path so the caller can feed it to `upload_elicitation_attachment`.
#[tauri::command]
fn stage_pasted_attachment(bytes: Vec<u8>, extension: String) -> Result<String, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    if (bytes.len() as u64) > claw_fleet_core::backend::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment too large: {} bytes (max {})",
            bytes.len(),
            claw_fleet_core::backend::MAX_ATTACHMENT_BYTES
        ));
    }
    let dir = std::env::temp_dir().join("fleet-pasted");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let ext = extension.trim_start_matches('.');
    let filename = if ext.is_empty() {
        format!("paste-{nanos}-{pid}.bin")
    } else {
        format!("paste-{nanos}-{pid}.{ext}")
    };
    let dest = dir.join(&filename);
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

// ── Plan approval (ExitPlanMode interception) ───────────────────────────────

#[tauri::command]
fn apply_plan_approval_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().apply_plan_approval_hook()
}

#[tauri::command]
fn remove_plan_approval_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_plan_approval_hook()
}

#[tauri::command]
fn list_pending_plan_approvals(
    state: tauri::State<AppState>,
) -> Vec<claw_fleet_core::plan_approval::PlanApprovalRequest> {
    state.backend.read().unwrap().list_pending_plan_approvals()
}

#[tauri::command]
fn respond_to_plan_approval(
    state: tauri::State<AppState>,
    id: String,
    decision: String,
    edited_plan: Option<String>,
    feedback: Option<String>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .respond_to_plan_approval(&id, &decision, edited_plan, feedback)
}

/// Read the last non-tool-use assistant message from a session, for guard context.
#[tauri::command]
fn get_guard_context(state: tauri::State<AppState>, session_id: String) -> String {
    // Find the session by ID and read its messages.
    let backend = state.backend.read().unwrap();
    let sessions = backend.list_sessions();
    let session = sessions.iter().find(|s| s.id == session_id);
    let Some(session) = session else {
        return String::new();
    };
    let messages = match backend.get_messages(&session.jsonl_path) {
        Ok(msgs) => msgs,
        Err(_) => return String::new(),
    };

    // Walk backwards to find the last assistant message with text content
    // (not just tool_use blocks).
    for msg in messages.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };

        // Collect text blocks, skip tool_use blocks.
        let text_parts: Vec<&str> = content
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect();

        if !text_parts.is_empty() {
            let combined = text_parts.join("\n");
            // Truncate to ~2000 chars for the LLM prompt.
            if combined.len() > 2000 {
                return format!("{}…", &combined[..2000]);
            }
            return combined;
        }
    }

    String::new()
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
fn set_locale(app: tauri::AppHandle, locale: String, state: tauri::State<AppState>) {
    let prev = std::mem::replace(&mut *state.locale.lock().unwrap(), locale.clone());
    let title = state.user_title.lock().unwrap().clone();
    reapply_interaction_mode_if_installed(&state, &title, Some(&locale));
    // Rebuild the app menu only if the language prefix actually changed, so
    // we don't churn the native menu on every startup call.
    let prev_prefix = prev.get(..2).unwrap_or("");
    let next_prefix = locale.get(..2).unwrap_or("");
    if prev_prefix != next_prefix {
        install_app_menu(&app);
    }
}

// ── Waiting alerts ──────────────────────────────────────────────────────────

#[tauri::command]
fn get_waiting_alerts(state: tauri::State<AppState>) -> Vec<backend::WaitingAlert> {
    state.backend.read().unwrap().get_waiting_alerts()
}

// ── Mascot quip generation ──────────────────────────────────────────────────

#[tauri::command]
async fn generate_mascot_quips(
    state: tauri::State<'_, AppState>,
    busy_titles: Vec<String>,
    done_titles: Vec<String>,
    locale: String,
) -> Result<claude_analyze::MascotQuips, String> {
    let cfg = state.llm_config.lock().unwrap().clone();
    Ok(tokio::task::spawn_blocking(move || {
        let provider = llm_provider::resolve_provider(&cfg.provider);
        match provider {
            Some(p) => claude_analyze::generate_mascot_quips(
                p.as_ref(), &cfg.standard_model, &busy_titles, &done_titles, &locale,
            ),
            None => claude_analyze::MascotQuips::default(),
        }
    })
    .await
    .unwrap_or_default())
}

// ── LLM provider commands ──────────────────────────────────────────────────

#[tauri::command]
fn list_llm_providers(state: tauri::State<AppState>) -> Vec<llm_provider::LlmProviderInfo> {
    state.cached_llm_providers.lock().unwrap().clone()
}

#[tauri::command]
fn get_llm_config(state: tauri::State<AppState>) -> llm_provider::LlmConfig {
    state.backend.read().unwrap().get_llm_config()
}

#[tauri::command]
fn set_llm_config(state: tauri::State<AppState>, config: llm_provider::LlmConfig) -> Result<(), String> {
    // Update both AppState (for background threads) and Backend.
    *state.llm_config.lock().unwrap() = config.clone();
    state.backend.read().unwrap().set_llm_config(config)
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// Lite portrait mode — shrink main window to phone-like portrait strip.
// We intentionally keep the native decorations (titleBarStyle: Overlay on
// macOS, default chrome elsewhere) because toggling set_decorations at
// runtime drops the Overlay style and the title bar can't be restored —
// that manifested as a broken title bar after exiting lite. Trade-off:
// traffic lights stay visible in lite mode, but we gain native rounded
// corners + correct restore.
#[tauri::command]
fn set_lite_mode(app: tauri::AppHandle, enabled: bool) {
    let Some(w) = app.get_webview_window("main") else { return };
    if enabled {
        let _ = w.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(
            300.0, 520.0,
        ))));
        let _ = w.set_size(tauri::Size::Logical(tauri::LogicalSize::new(340.0, 720.0)));
        if let Ok(Some(monitor)) = w.current_monitor() {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let screen_w = size.width as f64 / scale;
            let x = screen_w - 360.0;
            let y = 40.0;
            let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        }
    } else {
        let _ = w.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(
            900.0, 600.0,
        ))));
        let _ = w.set_size(tauri::Size::Logical(tauri::LogicalSize::new(1280.0, 820.0)));
        let _ = w.center();
    }
}

#[tauri::command]
fn toggle_tray_panel(_app: tauri::AppHandle, _visible: bool) {
    // No-op: custom tray panel removed; kept for frontend compat.
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

// ── Settings window ──────────────────────────────────────────────────────────

#[tauri::command]
fn open_settings_window(app: tauri::AppHandle, connection: Option<String>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }

    let mut path = String::from("settings.html");
    if let Some(conn) = connection.filter(|s| !s.is_empty()) {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        path.push_str("?connection=");
        path.push_str(&utf8_percent_encode(&conn, NON_ALPHANUMERIC).to_string());
    }

    let window = tauri::WebviewWindowBuilder::new(
        &app,
        "settings",
        tauri::WebviewUrl::App(path.into()),
    )
    .title("Settings")
    .inner_size(780.0, 640.0)
    .min_inner_size(560.0, 480.0)
    .center()
    .build()
    .map_err(|e| e.to_string())?;

    // Hide on close instead of destroying the WKWebView: tearing down a secondary
    // webview races with delayed WebKit main-thread work items (observed crash in
    // WebPageProxy::dispatchSetObscuredContentInsets on macOS 26.3.1).
    let hide_target = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide_target.hide();
        }
    });

    Ok(())
}

// ── Preview subwindow (lite-mode decision preview) ──────────────────────────

#[tauri::command]
fn open_preview_window(
    app: tauri::AppHandle,
    markdown: String,
    title: Option<String>,
) -> Result<(), String> {
    // If already open, just push new content via event and bring to front.
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.show();
        let _ = w.unminimize();
        let payload = serde_json::json!({
            "markdown": markdown,
            "title": title,
        });
        let _ = w.emit("preview://update", payload);
        return Ok(());
    }

    let mut path = String::from("preview.html");
    {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        path.push_str("?markdown=");
        path.push_str(&utf8_percent_encode(&markdown, NON_ALPHANUMERIC).to_string());
        if let Some(t) = title.as_deref().filter(|s| !s.is_empty()) {
            path.push_str("&title=");
            path.push_str(&utf8_percent_encode(t, NON_ALPHANUMERIC).to_string());
        }
    }

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        "preview",
        tauri::WebviewUrl::App(path.into()),
    )
    .title(title.as_deref().unwrap_or("Preview"))
    .inner_size(420.0, 520.0)
    .min_inner_size(280.0, 240.0)
    .resizable(true)
    .decorations(true)
    .always_on_top(true)
    .skip_taskbar(true);

    // Position beside the main window when we can; otherwise let Tauri pick.
    // Tauri's builder.position() takes logical coords, so convert physical
    // -> logical using the main window's scale factor (HiDPI correctness).
    if let Some(main) = app.get_webview_window("main") {
        let scale = main.scale_factor().unwrap_or(1.0);
        if let (Ok(pos), Ok(size)) = (main.outer_position(), main.outer_size()) {
            let x = (pos.x as f64 + size.width as f64) / scale + 8.0;
            let y = pos.y as f64 / scale;
            builder = builder.position(x, y);
        }
    }

    let window = builder.build().map_err(|e| e.to_string())?;

    // Same WKWebView teardown-race workaround as the settings window: hide
    // instead of destroying so queued WebKit work items can't dereference a
    // freed WebPageProxy.
    let hide_target = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide_target.hide();
        }
    });

    Ok(())
}

#[tauri::command]
fn update_preview_content(
    app: tauri::AppHandle,
    markdown: String,
    title: Option<String>,
) -> Result<(), String> {
    let Some(w) = app.get_webview_window("preview") else {
        return Ok(());
    };
    let payload = serde_json::json!({
        "markdown": markdown,
        "title": title,
    });
    w.emit("preview://update", payload)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn close_preview_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.close();
    }
    Ok(())
}

// ── Decision float window (shown when main is minimized) ─────────────────────

const DECISION_FLOAT_LABEL: &str = "decision-float";
const DECISION_FLOAT_W: f64 = 480.0;
const DECISION_FLOAT_H: f64 = 380.0;
const DECISION_FLOAT_BOTTOM_MARGIN: f64 = 64.0;

/// Logical top-left of the decision float window placed at the bottom-center
/// of whichever monitor currently contains the cursor. Falls back to the
/// primary monitor, then to (120, 120).
fn decision_float_target_position(app: &tauri::AppHandle) -> (f64, f64) {
    let cursor = app.cursor_position().ok();
    let monitors = app.available_monitors().unwrap_or_default();

    let chosen = cursor.and_then(|c| {
        monitors.iter().find(|m| {
            let pos = m.position();
            let size = m.size();
            let x0 = pos.x as f64;
            let y0 = pos.y as f64;
            let x1 = x0 + size.width as f64;
            let y1 = y0 + size.height as f64;
            c.x >= x0 && c.x < x1 && c.y >= y0 && c.y < y1
        })
    }).or_else(|| app.primary_monitor().ok().flatten().and_then(|_| monitors.first()));

    if let Some(mon) = chosen {
        let scale = mon.scale_factor();
        let mon_x = mon.position().x as f64 / scale;
        let mon_y = mon.position().y as f64 / scale;
        let mon_w = mon.size().width as f64 / scale;
        let mon_h = mon.size().height as f64 / scale;
        let x = mon_x + (mon_w - DECISION_FLOAT_W) / 2.0;
        let y = mon_y + mon_h - DECISION_FLOAT_H - DECISION_FLOAT_BOTTOM_MARGIN;
        (x, y)
    } else {
        (120.0, 120.0)
    }
}

#[tauri::command]
fn show_decision_float(
    app: tauri::AppHandle,
    snapshot: serde_json::Value,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    *state.decision_float_snapshot.lock().unwrap() = Some(snapshot);

    let (x, y) = decision_float_target_position(&app);

    if let Some(w) = app.get_webview_window(DECISION_FLOAT_LABEL) {
        let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(
        &app,
        DECISION_FLOAT_LABEL,
        tauri::WebviewUrl::App("decision-float.html".into()),
    )
    .title("Fleet Decision")
    .inner_size(DECISION_FLOAT_W, DECISION_FLOAT_H)
    .min_inner_size(360.0, 280.0)
    .position(x, y)
    .resizable(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(true)
    .build()
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
fn hide_decision_float(app: tauri::AppHandle, state: tauri::State<AppState>) {
    *state.decision_float_snapshot.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window(DECISION_FLOAT_LABEL) {
        let _ = w.hide();
    }
}

#[tauri::command]
fn get_decision_float_snapshot(state: tauri::State<AppState>) -> Option<serde_json::Value> {
    state.decision_float_snapshot.lock().unwrap().clone()
}

#[tauri::command]
fn is_main_window_minimized(app: tauri::AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_minimized().ok())
        .unwrap_or(false)
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
        RateLimited => "rate limited",
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

// ── App menu bar ────────────────────────────────────────────────────────────
//
// Builds the top-of-window (macOS) / in-window (Windows/Linux) menu bar.
// Custom items carry `menu-*` ids so they never collide with the tray menu's
// own ids. Predefined items (cut/copy/paste, quit, close, about…) are handled
// by the OS / webview directly and don't need a handler.
//
// Labels are locale-gated: when the frontend calls `set_locale`, we rebuild
// the menu so macOS/Win/Linux show the user's language.

struct MenuLabels {
    app_menu_title: &'static str,
    about_item: &'static str,
    settings: &'static str,
    check_updates: &'static str,
    services: &'static str,
    hide_self: &'static str,
    hide_others: &'static str,
    show_all: &'static str,
    quit: &'static str,

    file: &'static str,
    switch_connection: &'static str,
    daily_report: &'static str,
    close_window: &'static str,

    edit: &'static str,
    undo: &'static str,
    redo: &'static str,
    cut: &'static str,
    copy: &'static str,
    paste: &'static str,
    select_all: &'static str,

    view: &'static str,
    toggle_lite: &'static str,
    theme: &'static str,
    theme_system: &'static str,
    theme_light: &'static str,
    theme_dark: &'static str,
    reload: &'static str,
    fullscreen: &'static str,

    window: &'static str,
    minimize: &'static str,
    maximize: &'static str,

    help: &'static str,
    welcome: &'static str,
    mobile_access: &'static str,
    report_issue: &'static str,
}

fn menu_labels(locale: &str) -> MenuLabels {
    if locale.starts_with("zh") {
        MenuLabels {
            app_menu_title: "Claw Fleet",
            about_item: "关于 Claw Fleet",
            settings: "设置…",
            check_updates: "检查更新…",
            services: "服务",
            hide_self: "隐藏 Claw Fleet",
            hide_others: "隐藏其他",
            show_all: "全部显示",
            quit: "退出 Claw Fleet",

            file: "文件",
            switch_connection: "切换连接",
            daily_report: "每日报告",
            close_window: "关闭窗口",

            edit: "编辑",
            undo: "撤销",
            redo: "重做",
            cut: "剪切",
            copy: "复制",
            paste: "粘贴",
            select_all: "全选",

            view: "视图",
            toggle_lite: "切换轻量模式",
            theme: "主题",
            theme_system: "跟随系统",
            theme_light: "亮色",
            theme_dark: "暗色",
            reload: "重新加载",
            fullscreen: "进入全屏",

            window: "窗口",
            minimize: "最小化",
            maximize: "最大化",

            help: "帮助",
            welcome: "欢迎向导",
            mobile_access: "移动端接入",
            report_issue: "反馈问题…",
        }
    } else {
        MenuLabels {
            app_menu_title: "Claw Fleet",
            about_item: "About Claw Fleet",
            settings: "Settings…",
            check_updates: "Check for Updates…",
            services: "Services",
            hide_self: "Hide Claw Fleet",
            hide_others: "Hide Others",
            show_all: "Show All",
            quit: "Quit Claw Fleet",

            file: "File",
            switch_connection: "Switch Connection",
            daily_report: "Daily Report",
            close_window: "Close Window",

            edit: "Edit",
            undo: "Undo",
            redo: "Redo",
            cut: "Cut",
            copy: "Copy",
            paste: "Paste",
            select_all: "Select All",

            view: "View",
            toggle_lite: "Toggle Lite Mode",
            theme: "Theme",
            theme_system: "System",
            theme_light: "Light",
            theme_dark: "Dark",
            reload: "Reload",
            fullscreen: "Enter Full Screen",

            window: "Window",
            minimize: "Minimize",
            maximize: "Maximize",

            help: "Help",
            welcome: "Welcome",
            mobile_access: "Mobile Access",
            report_issue: "Report Issue…",
        }
    }
}

fn build_app_menu(
    app: &tauri::AppHandle,
    l: &MenuLabels,
) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    // ── App submenu (macOS shows as "Claw Fleet"; ignored on Win/Linux) ─
    let about_meta = AboutMetadataBuilder::new()
        .name(Some("Claw Fleet"))
        .version(Some(env!("CARGO_PKG_VERSION")))
        .website(Some("https://github.com/hoveychen/claw-fleet"))
        .website_label(Some("GitHub"))
        .build();
    let about = PredefinedMenuItem::about(app, Some(l.about_item), Some(about_meta))?;

    let app_submenu = SubmenuBuilder::new(app, l.app_menu_title)
        .item(&about)
        .separator()
        .item(
            &MenuItemBuilder::new(l.settings)
                .id("menu-settings")
                .accelerator("CmdOrCtrl+,")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.check_updates)
                .id("menu-check-updates")
                .build(app)?,
        )
        .separator()
        .services_with_text(l.services)
        .separator()
        .hide_with_text(l.hide_self)
        .hide_others_with_text(l.hide_others)
        .show_all_with_text(l.show_all)
        .separator()
        .quit_with_text(l.quit)
        .build()?;

    // ── File ────────────────────────────────────────────────────────────
    let file_submenu = SubmenuBuilder::new(app, l.file)
        .item(
            &MenuItemBuilder::new(l.switch_connection)
                .id("menu-switch-connection")
                .accelerator("CmdOrCtrl+Shift+C")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::new(l.daily_report)
                .id("menu-daily-report")
                .build(app)?,
        )
        .separator()
        .close_window_with_text(l.close_window)
        .build()?;

    // ── Edit (required for text inputs on macOS) ────────────────────────
    let edit_submenu = SubmenuBuilder::new(app, l.edit)
        .undo_with_text(l.undo)
        .redo_with_text(l.redo)
        .separator()
        .cut_with_text(l.cut)
        .copy_with_text(l.copy)
        .paste_with_text(l.paste)
        .separator()
        .select_all_with_text(l.select_all)
        .build()?;

    // ── View ────────────────────────────────────────────────────────────
    let theme_submenu = SubmenuBuilder::new(app, l.theme)
        .item(
            &MenuItemBuilder::new(l.theme_system)
                .id("menu-theme-system")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.theme_light)
                .id("menu-theme-light")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.theme_dark)
                .id("menu-theme-dark")
                .build(app)?,
        )
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, l.view)
        .item(
            &MenuItemBuilder::new(l.toggle_lite)
                .id("menu-toggle-lite")
                .accelerator("CmdOrCtrl+Shift+L")
                .build(app)?,
        )
        .item(&theme_submenu)
        .separator()
        .item(
            &MenuItemBuilder::new(l.reload)
                .id("menu-reload")
                .accelerator("CmdOrCtrl+R")
                .build(app)?,
        )
        .fullscreen_with_text(l.fullscreen)
        .build()?;

    // ── Window ──────────────────────────────────────────────────────────
    let window_submenu = SubmenuBuilder::new(app, l.window)
        .minimize_with_text(l.minimize)
        .maximize_with_text(l.maximize)
        .build()?;

    // ── Help ────────────────────────────────────────────────────────────
    let help_submenu = SubmenuBuilder::new(app, l.help)
        .item(
            &MenuItemBuilder::new(l.welcome)
                .id("menu-welcome")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.mobile_access)
                .id("menu-mobile-access")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::new(l.report_issue)
                .id("menu-report-issue")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.check_updates)
                .id("menu-check-updates-help")
                .build(app)?,
        )
        .build()?;

    MenuBuilder::new(app)
        .item(&app_submenu)
        .item(&file_submenu)
        .item(&edit_submenu)
        .item(&view_submenu)
        .item(&window_submenu)
        .item(&help_submenu)
        .build()
}

/// Build and install the app menu using the current locale stored in
/// AppState. Called from `setup` (initial build) and `set_locale` (rebuild).
fn install_app_menu(app: &tauri::AppHandle) {
    let locale = {
        let state = app.state::<AppState>();
        let guard = state.locale.lock().unwrap();
        guard.clone()
    };
    let labels = menu_labels(&locale);
    match build_app_menu(app, &labels) {
        Ok(menu) => {
            let _ = app.set_menu(menu);
        }
        Err(e) => {
            eprintln!("failed to build app menu: {e}");
        }
    }
}

/// Handle an event fired by the app menu (distinct from the tray menu).
/// Returns `true` if the id was recognised and handled.
fn handle_app_menu_event(app: &tauri::AppHandle, id: &str) -> bool {
    match id {
        "menu-settings" => {
            let _ = open_settings_window(app.clone(), None);
        }
        "menu-check-updates" | "menu-check-updates-help" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-check-updates", ());
        }
        "menu-switch-connection" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("switch-connection", ());
        }
        "menu-daily-report" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-daily-report", ());
        }
        "menu-toggle-lite" => {
            let _ = app.emit("menu-toggle-lite", ());
        }
        "menu-theme-system" => {
            let _ = app.emit("menu-theme", "system");
        }
        "menu-theme-light" => {
            let _ = app.emit("menu-theme", "light");
        }
        "menu-theme-dark" => {
            let _ = app.emit("menu-theme", "dark");
        }
        "menu-reload" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.eval("window.location.reload()");
            }
        }
        "menu-welcome" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-welcome", ());
        }
        "menu-mobile-access" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-mobile-access", ());
        }
        "menu-report-issue" => {
            use tauri_plugin_opener::OpenerExt;
            let _ = app
                .opener()
                .open_url("https://github.com/hoveychen/claw-fleet/issues", None::<&str>);
        }
        _ => return false,
    }
    true
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

// ── Mobile access commands ──────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MobileAccessInfo {
    running: bool,
    port: u16,
    token: String,
    tunnel_url: Option<String>,
    connected_clients: usize,
    cloudflared_available: bool,
    /// True while cloudflared is being downloaded / tunnel is being set up.
    setting_up: bool,
    /// Error message if tunnel setup failed.
    error: Option<String>,
}

#[tauri::command]
async fn enable_mobile_access(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<MobileAccessInfo, String> {
    // Stop existing server/tunnel if any.
    {
        let mut tunnel_guard = state.mobile_tunnel.lock().unwrap();
        if let Some(mut t) = tunnel_guard.take() {
            t.stop();
        }
        let mut server_guard = state.mobile_server.lock().unwrap();
        if let Some(mut s) = server_guard.take() {
            s.stop();
        }
    }

    // Generate a random auth token.
    let token: String = {
        let mut hasher = DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };

    // Pick an available port (bind to 0, get assigned port, release).
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("cannot find available port: {e}"))?;
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        port
    };

    // Start the embedded HTTP server (waits for bind confirmation).
    let backend = state.backend.clone();
    let server = embedded_server::EmbeddedServer::start(backend, port, token.clone())
        .map_err(|e| format!("embedded server failed: {e}"))?;

    log_debug(&format!("[mobile-access] server started on port {port}"));

    // Store server immediately so it's usable even while tunnel downloads.
    *state.mobile_server.lock().unwrap() = Some(server);

    // Mark setup as in-progress.
    state.mobile_setup_in_progress.store(true, std::sync::atomic::Ordering::Relaxed);

    // Download cloudflared (if needed) and start tunnel — in a blocking thread
    // so we don't freeze the UI.
    let mobile_tunnel = state.mobile_tunnel.clone();
    let setup_flag = state.mobile_setup_in_progress.clone();
    let app_for_progress = app.clone();
    let app_for_phase = app.clone();
    let tunnel_result = tokio::task::spawn_blocking(move || {
        // Notify frontend we're checking/downloading cloudflared.
        let _ = app_for_phase.emit("mobile-access-phase", "downloading");

        let progress_cb: tunnel::ProgressFn = Box::new(move |downloaded, total| {
            let _ = app_for_progress.emit("mobile-access-progress", serde_json::json!({
                "downloaded": downloaded,
                "total": total,
            }));
        });

        let binary = tunnel::find_or_download_cloudflared_with_progress(Some(progress_cb))
            .map_err(|e| e.to_string())?;

        // Notify frontend we're starting the tunnel.
        let _ = app_for_phase.emit("mobile-access-phase", "tunnel");

        tunnel::CloudflareTunnel::start_with_binary(&binary, port)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        setup_flag.store(false, std::sync::atomic::Ordering::Relaxed);
        format!("task failed: {e}")
    })?;

    // Setup done.
    state.mobile_setup_in_progress.store(false, std::sync::atomic::Ordering::Relaxed);

    let (tunnel_url, tunnel_error) = match &tunnel_result {
        Ok(t) => (Some(t.url().to_string()), None),
        Err(e) => {
            log_debug(&format!("[mobile-access] tunnel failed: {e}"));
            eprintln!("[mobile-access] tunnel failed: {e}");
            (None, Some(e.clone()))
        }
    };

    if let Ok(t) = tunnel_result {
        *mobile_tunnel.lock().unwrap() = Some(t);
    }

    let server_guard = state.mobile_server.lock().unwrap();
    let info = MobileAccessInfo {
        running: true,
        port,
        token,
        tunnel_url,
        connected_clients: server_guard.as_ref().map_or(0, |s| s.broadcaster().client_count()),
        cloudflared_available: tunnel::is_cloudflared_available(),
        setting_up: false,
        error: tunnel_error,
    };

    // Signal completion.
    let _ = app.emit("mobile-access-ready", &info);

    Ok(info)
}

#[tauri::command]
async fn disable_mobile_access(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut tunnel = state.mobile_tunnel.lock().unwrap().take();
    let mut server = state.mobile_server.lock().unwrap().take();
    state.mobile_setup_in_progress.store(false, std::sync::atomic::Ordering::Relaxed);

    // Stop in a blocking thread so we don't freeze the UI.
    tokio::task::spawn_blocking(move || {
        if let Some(ref mut t) = tunnel {
            t.stop();
        }
        if let Some(ref mut s) = server {
            s.stop();
        }
    })
    .await
    .map_err(|e| format!("stop failed: {e}"))?;

    Ok(())
}

#[tauri::command]
fn get_mobile_access_status(state: tauri::State<'_, AppState>) -> MobileAccessInfo {
    let server_guard = state.mobile_server.lock().unwrap();
    let tunnel_guard = state.mobile_tunnel.lock().unwrap();
    let setting_up = state.mobile_setup_in_progress.load(std::sync::atomic::Ordering::Relaxed);

    match server_guard.as_ref() {
        Some(server) => {
            let tunnel_url = tunnel_guard.as_ref().map(|t| t.url().to_string());
            MobileAccessInfo {
                running: true,
                port: server.port(),
                token: server.token().to_string(),
                tunnel_url,
                connected_clients: server.broadcaster().client_count(),
                cloudflared_available: tunnel::is_cloudflared_available(),
                setting_up,
                error: None,
            }
        }
        None => MobileAccessInfo {
            running: false,
            port: 0,
            token: String::new(),
            tunnel_url: None,
            connected_clients: 0,
            cloudflared_available: tunnel::is_cloudflared_available(),
            setting_up,
            error: None,
        },
    }
}

/// Returns the QR code data for mobile pairing.
/// The QR encodes a URL: `https://xxx.trycloudflare.com/mobile?token=TOKEN`
/// - Scanned by Claw Fleet app → parsed as connection URL
/// - Scanned by generic QR reader → opens landing page in browser
#[tauri::command]
fn get_mobile_qr_data(state: tauri::State<'_, AppState>) -> Option<String> {
    let server_guard = state.mobile_server.lock().unwrap();
    let tunnel_guard = state.mobile_tunnel.lock().unwrap();

    let server = server_guard.as_ref()?;
    let tunnel = tunnel_guard.as_ref()?;

    // URL format that serves both as a web page and as app connection data.
    Some(format!("{}/mobile?token={}", tunnel.url(), server.token()))
}

// ── App setup ────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
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
        .plugin(tauri_plugin_dialog::init());

    builder.manage(AppState {
            // NullBackend is a placeholder; replaced with LocalBackend in setup().
            backend: Arc::new(RwLock::new(Box::new(NullBackend) as Box<dyn Backend>)),
            locale: Arc::new(Mutex::new("en".to_string())),
            notification_mode: Arc::new(Mutex::new("user_action".to_string())),
            user_title: Arc::new(Mutex::new(String::new())),
            cached_sessions: Arc::new(Mutex::new(Vec::new())),
            cached_usage: Arc::new(Mutex::new(Vec::new())),
            tray_fingerprint: Arc::new(Mutex::new(0)),
            tray_last_click: Arc::new(Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(600))),
            tray_rebuild_pending: Arc::new(Mutex::new(false)),
            llm_config: Arc::new(Mutex::new(llm_provider::LlmConfig::default())),
            cached_llm_providers: Arc::new(Mutex::new(Vec::new())),
            mobile_server: Arc::new(Mutex::new(None)),
            mobile_tunnel: Arc::new(Mutex::new(None)),
            mobile_setup_in_progress: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            decision_float_snapshot: Arc::new(Mutex::new(None)),
        })
        .setup(move |app| {
            // Replace NullBackend with the real LocalBackend now that AppHandle
            // is available.
            {
                let state = app.state::<AppState>();
                let locale = state.locale.clone();
                let llm_cfg = state.llm_config.clone();

                // Build the agent source registry from config (~/.fleet/fleet-sources.json).
                let sources = agent_source::build_sources();

                let local = local_backend::LocalBackend::new(
                    app.handle().clone(),
                    locale,
                    llm_cfg,
                    sources,
                );
                *state.backend.write().unwrap() = Box::new(local);

                // Pre-fetch LLM provider info in background so Settings opens instantly.
                let cached = state.cached_llm_providers.clone();
                std::thread::spawn(move || {
                    let infos = llm_provider::all_provider_infos();
                    *cached.lock().unwrap() = infos;
                });
            }

            // ── SSE forwarding for mobile access ──────────────────────────
            // Listen for sessions-updated Tauri events and broadcast them to
            // any connected SSE mobile clients.
            {
                let mobile_server = app.state::<AppState>().mobile_server.clone();
                app.listen("sessions-updated", move |event| {
                    let guard = mobile_server.lock().unwrap();
                    if let Some(ref server) = *guard {
                        if server.broadcaster().client_count() > 0 {
                            // event.payload() is already a JSON string.
                            server.broadcaster().broadcast("sessions-updated", event.payload());
                        }
                    }
                });
            }
            {
                let mobile_server = app.state::<AppState>().mobile_server.clone();
                app.listen("waiting-alert", move |event| {
                    let guard = mobile_server.lock().unwrap();
                    if let Some(ref server) = *guard {
                        if server.broadcaster().client_count() > 0 {
                            server.broadcaster().broadcast("waiting-alert", event.payload());
                        }
                    }
                });
            }

            // Truncate the hook events file if it has grown too large.
            hooks::maybe_truncate_events_file();

            // ── Audit pattern updates ───────────────────────────────────────
            // Seed local patterns from bundled resource (first run or app
            // upgrade), then start the daily background updater.
            desktop_pattern_update::bootstrap_patterns(app.handle());
            pattern_update::start_background_updater();

            // Background usage refresh removed — the frontend's periodic
            // `get_source_usage` / `get_account_info` calls now update the
            // cached tray summaries as a side-effect, avoiding duplicate
            // network requests that could hit rate limits.

            // ── App menu bar ─────────────────────────────────────────────────
            // Register the main app menu (File / Edit / View / Window / Help …).
            // The global menu-event handler below dispatches custom items with
            // `menu-*` ids; tray items keep their own (tray-scoped) handler.
            // Labels come from the current locale (AppState::locale), which is
            // synced from the frontend on mount via `set_locale`; the menu is
            // rebuilt there whenever the user switches language.
            install_app_menu(app.handle());
            app.handle().on_menu_event(|app, event| {
                let id = event.id().as_ref().to_string();
                if id.starts_with("menu-") {
                    handle_app_menu_event(app, &id);
                }
            });

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
                .tooltip("Claw Fleet")
                .on_tray_icon_event(|tray, event| {
                    // Record click timestamp so we can defer tray menu rebuilds
                    // while the menu is open.
                    if let tauri::tray::TrayIconEvent::Click { button, button_state, .. } = &event {
                        if matches!(button_state, tauri::tray::MouseButtonState::Up) {
                            let app = tray.app_handle();
                            let state = app.state::<AppState>();
                            *state.tray_last_click.lock().unwrap() = std::time::Instant::now();

                            // Left-click: show main window
                            if matches!(button, tauri::tray::MouseButton::Left) {
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                            }
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

            // ── macOS vibrancy (frosted glass) ────────────────────────────
            #[cfg(target_os = "macos")]
            {
                use tauri::window::{Color, Effect, EffectState, EffectsBuilder};

                if let Some(main_win) = app.get_webview_window("main") {
                    let _ = main_win.set_background_color(Some(Color(0, 0, 0, 0)));
                    let effects = EffectsBuilder::new()
                        .effect(Effect::Sidebar)
                        .state(EffectState::Active)
                        .build();
                    let _ = main_win.set_effects(effects);
                }
            }

            // ── Main window minimize watcher ─────────────────────────────────
            // Emit a frontend event whenever the main window's minimized state
            // may have changed, so the decision-float window can be shown /
            // hidden accordingly. Tauri has no dedicated "minimized" event, so
            // we re-check on Resized and Focused.
            if let Some(main_win) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                main_win.on_window_event(move |event| {
                    use tauri::WindowEvent;
                    match event {
                        WindowEvent::Resized(_) | WindowEvent::Focused(_) => {
                            if let Some(w) = handle.get_webview_window("main") {
                                let minimized = w.is_minimized().unwrap_or(false);
                                let _ = handle.emit(
                                    "main-window-minimize-state-changed",
                                    minimized,
                                );
                            }
                        }
                        _ => {}
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
            get_session_todos,
            get_audit_events,
            get_audit_rules,
            set_audit_rule_enabled,
            save_custom_audit_rule,
            delete_custom_audit_rule,
            suggest_audit_rules,
            check_pattern_update,
            get_pattern_info,
            start_watching_session,
            stop_watching_session,
            get_account_info,
            get_log_path,
            get_platform,
            check_app_version,
            get_app_version,
            kill_session,
            kill_workspace_sessions,
            resume_rate_limited_session,
            get_auto_resume_config,
            set_auto_resume_config,
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
            apply_guard_hook,
            remove_guard_hook,
            respond_to_guard,
            analyze_guard_command,
            get_guard_context,
            apply_elicitation_hook,
            remove_elicitation_hook,
            apply_interaction_mode,
            remove_interaction_mode,
            respond_to_elicitation,
            upload_elicitation_attachment,
            stage_pasted_attachment,
            apply_plan_approval_hook,
            remove_plan_approval_hook,
            list_pending_plan_approvals,
            respond_to_plan_approval,
            generate_mascot_quips,
            list_llm_providers,
            get_llm_config,
            set_llm_config,
            get_sources_config,
            set_source_enabled,
            restart_app,
            get_notification_mode,
            set_notification_mode,
            get_user_title,
            set_user_title,
            open_notification_settings,
            show_main_window,
            set_lite_mode,
            toggle_tray_panel,
            quit_app,
            open_settings_window,
            open_preview_window,
            update_preview_content,
            close_preview_window,
            show_decision_float,
            hide_decision_float,
            get_decision_float_snapshot,
            is_main_window_minimized,
            get_tts_voices,
            speak_text,
            speak_text_say,
            get_daily_report,
            list_daily_report_stats,
            generate_daily_report,
            generate_daily_report_ai_summary,
            generate_daily_report_lessons,
            append_lesson_to_claude_md,
            enable_mobile_access,
            disable_mobile_access,
            get_mobile_access_status,
            get_mobile_qr_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
