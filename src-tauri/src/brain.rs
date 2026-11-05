//! 두뇌: Claude Messages API + 도구 사용 루프.
//! 대화 기록은 앱이 켜져 있는 동안 메모리에만 둔다.

use crate::tools::{self, Toolbox};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio::sync::Mutex;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const MAX_TOOL_ROUNDS: usize = 6;
const MAX_HISTORY: usize = 30;

pub struct Brain {
    client: reqwest::Client,
    api_key: Option<String>,
    model: String,
    history: Mutex<Vec<Value>>,
    toolbox: Toolbox,
}

impl Brain {
    pub fn new(api_key: Option<String>, model: String, toolbox: Toolbox) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.filter(|k| !k.trim().is_empty()),
            model,
            history: Mutex::new(Vec::new()),
            toolbox,
        }
    }

    pub fn has_key(&self) -> bool {
        self.api_key.is_some()
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn toolbox(&self) -> &Toolbox {
        &self.toolbox
    }

    pub async fn reset(&self) {
        self.history.lock().await.clear();
    }

    pub async fn ask(&self, app: &AppHandle, text: String) -> Result<String> {
        let key = self.api_key.as_deref().context(
            "ANTHROPIC_API_KEY가 없습니다. 프로젝트 폴더의 .env 파일에 키를 넣어 주세요",
        )?;

        let mut hist = self.history.lock().await;
        let rollback = hist.len();
        hist.push(json!({ "role": "user", "content": text }));

        let result = self.run_loop(app, key, &mut hist).await;
        if result.is_err() {
            // 실패한 턴은 기록에서 지워 다음 요청이 깨지지 않게 한다
            hist.truncate(rollback);
        }
        trim(&mut hist);
        result
    }

    async fn run_loop(&self, app: &AppHandle, key: &str, hist: &mut Vec<Value>) -> Result<String> {
        for _ in 0..MAX_TOOL_ROUNDS {
            let body = json!({
                "model": self.model,
                "max_tokens": 700,
                "system": system_prompt(&self.toolbox),
                "tools": tools::schemas(&self.toolbox),
                "messages": hist,
            });

            let resp = self
                .client
                .post(API_URL)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .context("Claude 서버에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요")?;

            let status = resp.status();
            let v: Value = resp.json().await.context("Claude 응답을 읽지 못했습니다")?;
            if !status.is_success() {
                let msg = v["error"]["message"].as_str().unwrap_or("알 수 없는 오류");
                bail!("Claude API 오류 ({status}): {msg}");
            }

            let content = v["content"].clone();
            hist.push(json!({ "role": "assistant", "content": content.clone() }));

            let blocks = content.as_array().cloned().unwrap_or_default();
            let tool_uses: Vec<&Value> =
                blocks.iter().filter(|b| b["type"] == "tool_use").collect();

            if v["stop_reason"] == "tool_use" && !tool_uses.is_empty() {
                let mut results = Vec::with_capacity(tool_uses.len());
                for u in tool_uses {
                    let name = u["name"].as_str().unwrap_or_default();
                    let out = tools::run(app, &self.toolbox, name, &u["input"]).await;
                    let (text, is_error) = match out {
                        Ok(s) => (s, false),
                        Err(e) => (format!("{e:#}"), true),
                    };
                    results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": u["id"],
                        "content": text,
                        "is_error": is_error,
                    }));
                }
                hist.push(json!({ "role": "user", "content": results }));
                continue;
            }

            let reply: String = blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(reply.trim().to_string());
        }
        bail!("도구 호출이 너무 길어져서 멈췄습니다")
    }
}

/// 오래된 대화를 지운다. 도구 호출/결과 쌍이 끊기지 않도록 사용자의 일반 메시지에서만 자른다.
fn trim(hist: &mut Vec<Value>) {
    while hist.len() > MAX_HISTORY {
        hist.remove(0);
        while !hist.is_empty() && !(hist[0]["role"] == "user" && hist[0]["content"].is_string()) {
            hist.remove(0);
        }
    }
}

fn system_prompt(tb: &Toolbox) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d (%a) %H:%M");
    format!(
        "너는 토니 스타크의 AI 비서 J.A.R.V.I.S.다. 사용자를 '주인님'이라 부르고, 침착하고 정중하며 \
살짝 위트 있는 영국 집사 같은 말투의 한국어로 답한다.\n\
답변은 음성으로 읽히므로 보통 1~3문장으로 짧게, 마크다운·이모지·목록 기호 없이 말하듯 쓴다. \
숫자와 단위는 읽기 쉽게 풀어 쓴다.\n\
사용자의 말은 음성 인식 결과라 오타가 있을 수 있으니 의도를 추론하되, 애매하면 짧게 되묻는다.\n\
PC를 다룰 때는 반드시 도구를 쓰고, 도구가 실패하면 이유를 짧게 알린다. 할 수 없는 일은 솔직히 말한다.\n\
열 수 있는 앱: {apps}\n\
현재 시각: {now}",
        apps = tb.app_names().join(", ")
    )
}
