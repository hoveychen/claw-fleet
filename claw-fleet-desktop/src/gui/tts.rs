use super::*;

// ── TTS via Microsoft Edge TTS ───────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub(crate) struct TtsVoice {
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
pub(crate) async fn get_tts_voices(locale: String) -> Vec<TtsVoice> {
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
    let mut cmd = claw_fleet_core::process_util::command("say");
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
pub(crate) async fn speak_text(
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
pub(crate) fn speak_text_say(text: String, voice: Option<String>, locale: Option<String>) {
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


