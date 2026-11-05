//! 로컬 Whisper 음성 인식 (whisper.cpp 바인딩 whisper-rs).
//! 모델은 처음 말할 때 한 번만 메모리에 올리고 계속 재사용한다.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct Stt {
    model_path: PathBuf,
    language: String,
    ctx: Arc<OnceLock<WhisperContext>>,
}

impl Stt {
    pub fn new(model_path: PathBuf, language: String) -> Self {
        Self {
            model_path,
            language,
            ctx: Arc::new(OnceLock::new()),
        }
    }

    pub fn model_path(&self) -> &PathBuf {
        &self.model_path
    }

    pub fn is_loaded(&self) -> bool {
        self.ctx.get().is_some()
    }

    /// 16kHz mono f32 음성을 텍스트로 바꾼다. CPU를 오래 쓰므로 블로킹 스레드에서 돈다.
    pub async fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        // 0.3초보다 짧으면 버튼을 잘못 누른 것으로 본다
        if audio.len() < (crate::audio::WHISPER_RATE as usize * 3) / 10 {
            return Ok(String::new());
        }
        let path = self.model_path.clone();
        let lang = self.language.clone();
        let cell = self.ctx.clone();

        tauri::async_runtime::spawn_blocking(move || -> Result<String> {
            let ctx = load(&cell, &path)?;
            let mut state = ctx
                .create_state()
                .context("Whisper 상태를 만들지 못했습니다")?;

            let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            p.set_language(Some(&lang));
            p.set_translate(false);
            p.set_no_context(true);
            p.set_suppress_blank(true);
            p.set_print_progress(false);
            p.set_print_realtime(false);
            p.set_print_special(false);
            p.set_print_timestamps(false);
            let threads = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .min(8);
            p.set_n_threads(threads as i32);
            // 호출어와 고유명사를 잘 받아적도록 힌트를 준다
            p.set_initial_prompt("자비스, 오늘 일정 알려줘. 크롬 열어줘. 타이머 3분.");

            state.full(p, &audio).context("음성 인식에 실패했습니다")?;
            let n = state
                .full_n_segments()
                .context("세그먼트 수를 읽지 못했습니다")?;
            let mut text = String::new();
            for i in 0..n {
                text.push_str(
                    &state
                        .full_get_segment_text(i)
                        .context("세그먼트를 읽지 못했습니다")?,
                );
            }
            Ok(clean(&text))
        })
        .await?
    }
}

fn load<'a>(cell: &'a OnceLock<WhisperContext>, path: &PathBuf) -> Result<&'a WhisperContext> {
    if let Some(ctx) = cell.get() {
        return Ok(ctx);
    }
    if !path.exists() {
        bail!(
            "Whisper 모델 파일이 없습니다: {}\nREADME의 '모델 받기' 단계를 따라 ggml 모델을 내려받아 주세요",
            path.display()
        );
    }
    let p = path
        .to_str()
        .context("모델 경로에 쓸 수 없는 문자가 있습니다")?;
    let ctx = WhisperContext::new_with_params(p, WhisperContextParameters::default())
        .context("Whisper 모델을 불러오지 못했습니다")?;
    let _ = cell.set(ctx);
    Ok(cell.get().expect("just set"))
}

/// Whisper가 무음일 때 내뱉는 흔한 환각 문구를 걸러낸다.
fn clean(s: &str) -> String {
    let t = s.trim();
    const NOISE: [&str; 5] = [
        "[BLANK_AUDIO]",
        "(음악)",
        "[음악]",
        "시청해주셔서 감사합니다",
        "구독과 좋아요",
    ];
    if NOISE.iter().any(|n| t.contains(n)) && t.chars().count() < 30 {
        return String::new();
    }
    t.to_string()
}
