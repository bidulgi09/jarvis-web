// 릴리스 빌드에서 윈도우 콘솔 창 숨김
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod brain;
mod memory;
mod stt;
mod tools;

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, State};

struct AppState {
    recording: Mutex<Option<audio::Recording>>,
    stt: stt::Stt,
    brain: brain::Brain,
}

#[derive(Serialize)]
struct Status {
    api_key: bool,
    model: String,
    whisper_model: String,
    whisper_model_found: bool,
    whisper_loaded: bool,
    apps: Vec<String>,
}

#[tauri::command]
fn status(state: State<'_, AppState>) -> Status {
    let wm = state.stt.model_path();
    Status {
        api_key: state.brain.has_key(),
        model: state.brain.model().to_string(),
        whisper_model: wm
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        whisper_model_found: wm.exists(),
        whisper_loaded: state.stt.is_loaded(),
        apps: state
            .brain
            .toolbox()
            .app_names()
            .into_iter()
            .map(String::from)
            .collect(),
    }
}

/// 말하기 시작 (누르는 순간)
#[tauri::command]
fn start_listening(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut slot = state
        .recording
        .lock()
        .map_err(|_| "녹음 상태 잠금 실패".to_string())?;
    if slot.is_none() {
        *slot = Some(audio::Recording::start(app).map_err(|e| format!("{e:#}"))?);
    }
    Ok(())
}

/// 말하기 끝 (떼는 순간) → Whisper로 받아적은 문장을 돌려준다
#[tauri::command]
async fn stop_listening(state: State<'_, AppState>) -> Result<String, String> {
    let rec = state
        .recording
        .lock()
        .map_err(|_| "녹음 상태 잠금 실패".to_string())?
        .take();
    let Some(rec) = rec else {
        return Ok(String::new());
    };
    let samples = rec.finish().map_err(|e| format!("{e:#}"))?;
    state
        .stt
        .transcribe(samples)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn ask(app: AppHandle, state: State<'_, AppState>, text: String) -> Result<String, String> {
    state
        .brain
        .ask(&app, text)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// 기록 네트워크 화면용: 저장된 기록 전체
#[tauri::command]
fn list_memories(state: State<'_, AppState>) -> Vec<memory::Record> {
    state.brain.toolbox().memory.all()
}

#[tauri::command]
fn delete_memory(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .brain
        .toolbox()
        .memory
        .remove(&id)
        .map_err(|e| format!("{e:#}"))?;
    let _ = tauri::Emitter::emit(&app, "memory-changed", serde_json::json!({}));
    Ok(())
}

#[tauri::command]
async fn reset(state: State<'_, AppState>) -> Result<(), String> {
    state.brain.reset().await;
    Ok(())
}

/// 상대 경로는 .env가 있는 폴더(프로젝트 루트) 기준으로 푼다
fn resolve(base: &Path, p: &str) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}

fn main() {
    let base = dotenvy::dotenv()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();

    // .env에 키만 있고 값이 비어 있으면 기본값을 쓴다
    let env = |k: &str, d: &str| {
        std::env::var(k)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| d.to_string())
    };

    let stt = stt::Stt::new(
        resolve(&base, &env("WHISPER_MODEL", "models/ggml-small.bin")),
        env("WHISPER_LANG", "ko"),
    );
    let toolbox = tools::Toolbox::load(&resolve(&base, &env("JARVIS_APPS", "apps.json")));
    let provider = brain::Provider::from_env(&env("JARVIS_PROVIDER", "groq"));
    let brain = brain::Brain::new(
        provider,
        std::env::var(provider.key_var()).ok(),
        env("JARVIS_MODEL", provider.default_model()),
        toolbox,
    );

    tauri::Builder::default()
        .manage(AppState {
            recording: Mutex::new(None),
            stt,
            brain,
        })
        .invoke_handler(tauri::generate_handler![
            status,
            start_listening,
            stop_listening,
            ask,
            reset,
            list_memories,
            delete_memory
        ])
        .run(tauri::generate_context!())
        .expect("자비스를 시작하지 못했습니다");
}
