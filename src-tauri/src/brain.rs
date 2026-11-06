//! 두뇌: Groq(무료, 기본) / Gemini / Claude + 도구 사용 루프.
//! 시스템 프롬프트에 최근 기록 제목을 넣어, 새 기록을 기존 기록과 이어 줄 수 있게 한다.
//! 대화 기록은 앱이 켜져 있는 동안 메모리에만 둔다.

use crate::tools::{self, Toolbox};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use tauri::AppHandle;
use tokio::sync::Mutex;

const CLAUDE_URL: &str = "https://api.anthropic.com/v1/messages";
const GROQ_URL: &str = "https://api.groq.com/openai/v1/chat/completions";
const GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const MAX_TOOL_ROUNDS: usize = 6;
const MAX_HISTORY: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Groq,
    Gemini,
    Claude,
}

impl Provider {
    pub fn from_env(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Provider::Claude,
            "gemini" => Provider::Gemini,
            _ => Provider::Groq,
        }
    }
    pub fn key_var(self) -> &'static str {
        match self {
            Provider::Groq => "GROQ_API_KEY",
            Provider::Gemini => "GEMINI_API_KEY",
            Provider::Claude => "ANTHROPIC_API_KEY",
        }
    }
    pub fn default_model(self) -> &'static str {
        match self {
            Provider::Groq => "openai/gpt-oss-120b",
            Provider::Gemini => "gemini-3.5-flash-lite",
            Provider::Claude => "claude-haiku-5-5",
        }
    }
}

pub struct Brain {
    client: reqwest::Client,
    provider: Provider,
    api_key: Option<String>,
    model: String,
    history: Mutex<Vec<Value>>,
    toolbox: Toolbox,
}

impl Brain {
    pub fn new(
        provider: Provider,
        api_key: Option<String>,
        model: String,
        toolbox: Toolbox,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            provider,
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
        let key = self.api_key.as_deref().with_context(|| {
            format!(
                "{}가 없습니다. 프로젝트 폴더의 .env 파일에 키를 넣어 주세요",
                self.provider.key_var()
            )
        })?;

        let mut hist = self.history.lock().await;
        let rollback = hist.len();
        let result = match self.provider {
            Provider::Groq => {
                hist.push(json!({ "role": "user", "content": text }));
                self.groq_loop(app, key, &mut hist).await
            }
            Provider::Gemini => {
                hist.push(json!({ "role": "user", "parts": [{ "text": text }] }));
                self.gemini_loop(app, key, &mut hist).await
            }
            Provider::Claude => {
                hist.push(json!({ "role": "user", "content": text }));
                self.claude_loop(app, key, &mut hist).await
            }
        };
        if result.is_err() {
            // 실패한 턴은 기록에서 지워 다음 요청이 깨지지 않게 한다
            hist.truncate(rollback);
        }
        trim(&mut hist);
        result
    }

    // ---------------- Groq (OpenAI 호환) ----------------

    async fn groq_loop(&self, app: &AppHandle, key: &str, hist: &mut Vec<Value>) -> Result<String> {
        let tools: Vec<Value> = tools::schemas(&self.toolbox)
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|t| {
                json!({ "type": "function", "function": {
                    "name": t["name"], "description": t["description"], "parameters": t["input_schema"] } })
            })
            .collect();

        for _ in 0..MAX_TOOL_ROUNDS {
            let mut messages =
                vec![json!({ "role": "system", "content": system_prompt(&self.toolbox) })];
            messages.extend(hist.iter().cloned());
            let mut body = json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto",
                "max_completion_tokens": 4096,
                "temperature": 0.6,
            });
            if self.model.starts_with("openai/gpt-oss") {
                body["reasoning_effort"] = json!("low");
            }

            // 무료 한도(429)나 서버 바쁨(5xx)이면 잠깐 기다렸다 최대 3번까지
            let mut attempt = 0u32;
            let v: Value = loop {
                let resp = self
                    .client
                    .post(GROQ_URL)
                    .bearer_auth(key)
                    .json(&body)
                    .send()
                    .await
                    .context("Groq 서버에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요")?;
                let status = resp.status();
                let v: Value = resp.json().await.context("Groq 응답을 읽지 못했습니다")?;
                if status.is_success() {
                    break v;
                }
                let msg = v["error"]["message"].as_str().unwrap_or("알 수 없는 오류");
                let code = status.as_u16();
                let tool_fail = code == 400 && v["error"]["code"] == "tool_use_failed";
                if (code == 429 || code >= 500 || tool_fail) && attempt < 2 {
                    attempt += 1;
                    let wait = if code == 429 {
                        2500
                    } else {
                        600 * attempt as u64
                    };
                    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                    continue;
                }
                if code == 401 {
                    bail!("Groq API 키가 올바르지 않습니다. .env의 GROQ_API_KEY를 확인해 주세요");
                }
                if code == 429 {
                    bail!("Groq 무료 한도에 걸렸습니다. 1분쯤 뒤에 다시 말씀해 주세요. 계속되면 JARVIS_MODEL을 openai/gpt-oss-20b로 바꿔 보세요");
                }
                bail!("Groq API 오류 ({status}): {msg}");
            };

            let m = &v["choices"][0]["message"];
            let calls = m["tool_calls"].as_array().cloned().unwrap_or_default();
            if !calls.is_empty() {
                hist.push(
                    json!({ "role": "assistant", "content": m["content"], "tool_calls": calls }),
                );
                for c in &calls {
                    let name = c["function"]["name"].as_str().unwrap_or_default();
                    let args: Value = c["function"]["arguments"]
                        .as_str()
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or_else(|| json!({}));
                    let out = match tools::run(app, &self.toolbox, name, &args).await {
                        Ok(s) => s,
                        Err(e) => format!("오류: {e:#}"),
                    };
                    hist.push(json!({ "role": "tool", "tool_call_id": c["id"], "content": out }));
                }
                continue;
            }

            let reply = strip_think(m["content"].as_str().unwrap_or_default());
            if reply.is_empty() {
                bail!("Groq가 빈 답을 보냈습니다. 다시 말씀해 주세요");
            }
            hist.push(json!({ "role": "assistant", "content": reply }));
            return Ok(reply);
        }
        bail!("도구 호출이 너무 길어져서 멈췄습니다")
    }

    // ---------------- Gemini ----------------

    async fn gemini_loop(
        &self,
        app: &AppHandle,
        key: &str,
        hist: &mut Vec<Value>,
    ) -> Result<String> {
        let url = format!("{GEMINI_URL}/{}:generateContent", self.model);
        for _ in 0..MAX_TOOL_ROUNDS {
            let body = json!({
                "systemInstruction": { "parts": [{ "text": system_prompt(&self.toolbox) }] },
                "contents": hist,
                "tools": [{ "functionDeclarations": gemini_declarations(&self.toolbox) }],
                "generationConfig": { "maxOutputTokens": 8192 },
            });

            // 무료 모델은 과부하(5xx)나 분당 한도(429)가 잦아서 잠깐 기다렸다 최대 3번까지 다시 묻는다
            let mut attempt = 0u32;
            let v: Value = loop {
                let resp = self
                    .client
                    .post(&url)
                    .header("x-goog-api-key", key)
                    .json(&body)
                    .send()
                    .await
                    .context("Gemini 서버에 연결하지 못했습니다. 인터넷 연결을 확인해 주세요")?;

                let status = resp.status();
                let v: Value = resp.json().await.context("Gemini 응답을 읽지 못했습니다")?;
                if status.is_success() {
                    break v;
                }
                let msg = v["error"]["message"].as_str().unwrap_or("알 수 없는 오류");
                let code = status.as_u16();
                let retryable = code == 429 || code >= 500;
                if retryable && attempt < 2 {
                    attempt += 1;
                    let wait = if code == 429 {
                        2000
                    } else {
                        700 * (attempt * attempt) as u64
                    };
                    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                    continue;
                }
                if code == 429 {
                    bail!("Gemini 무료 사용량 한도에 걸렸습니다. 1분쯤 뒤에 다시 말씀해 주세요");
                }
                if code >= 500 {
                    bail!("Gemini 서버가 지금 바쁩니다 ({status}). 잠시 후 다시 말씀해 주세요. 계속되면 .env의 JARVIS_MODEL을 gemini-3.1-flash-lite로 바꿔 보세요");
                }
                if matches!(code, 401 | 403)
                    || (code == 400 && msg.to_lowercase().contains("api key"))
                {
                    bail!("Gemini가 API 키를 거절했습니다 ({status}). .env의 GEMINI_API_KEY를 확인해 주세요. 서버 메시지: {msg}");
                }
                bail!("Gemini API 오류 ({status}): {msg}");
            };

            let parts = v["candidates"][0]["content"]["parts"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if parts.is_empty() {
                bail!("Gemini가 빈 답을 보냈습니다. 다시 말씀해 주세요");
            }
            // thoughtSignature가 붙어 있을 수 있으므로 모델 parts는 그대로 기록한다
            hist.push(json!({ "role": "model", "parts": parts.clone() }));

            let calls: Vec<&Value> = parts
                .iter()
                .filter(|p| p.get("functionCall").is_some())
                .collect();
            if !calls.is_empty() {
                let mut out = Vec::with_capacity(calls.len());
                for p in calls {
                    let fc = &p["functionCall"];
                    let name = fc["name"].as_str().unwrap_or_default();
                    let args = if fc["args"].is_object() {
                        fc["args"].clone()
                    } else {
                        json!({})
                    };
                    let response = match tools::run(app, &self.toolbox, name, &args).await {
                        Ok(s) => json!({ "result": s }),
                        Err(e) => json!({ "error": format!("{e:#}") }),
                    };
                    let mut fr = json!({ "name": name, "response": response });
                    if let Some(id) = fc.get("id") {
                        fr["id"] = id.clone();
                    }
                    out.push(json!({ "functionResponse": fr }));
                }
                hist.push(json!({ "role": "user", "parts": out }));
                continue;
            }

            let reply: String = parts
                .iter()
                .filter(|p| p["thought"] != true)
                .filter_map(|p| p["text"].as_str())
                .collect();
            return Ok(reply.trim().to_string());
        }
        bail!("도구 호출이 너무 길어져서 멈췄습니다")
    }

    // ---------------- Claude ----------------

    async fn claude_loop(
        &self,
        app: &AppHandle,
        key: &str,
        hist: &mut Vec<Value>,
    ) -> Result<String> {
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
                .post(CLAUDE_URL)
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

/// Claude 형식 도구 스키마를 Gemini functionDeclarations로 바꾼다.
/// 인자가 없는 도구는 parameters를 빼야 하고(빈 OBJECT는 거절됨), minimum/maximum은 지운다(값은 도구에서 직접 제한).
fn gemini_declarations(tb: &Toolbox) -> Value {
    let decls: Vec<Value> = tools::schemas(tb)
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|t| {
            let mut d = Map::new();
            d.insert("name".into(), t["name"].clone());
            d.insert("description".into(), t["description"].clone());
            let mut params = t["input_schema"].clone();
            strip_bounds(&mut params);
            let has_props = params["properties"]
                .as_object()
                .is_some_and(|p| !p.is_empty());
            if has_props {
                d.insert("parameters".into(), params);
            }
            Value::Object(d)
        })
        .collect();
    Value::Array(decls)
}

fn strip_bounds(v: &mut Value) {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("minimum");
        obj.remove("maximum");
        for child in obj.values_mut() {
            strip_bounds(child);
        }
    } else if let Some(arr) = v.as_array_mut() {
        arr.iter_mut().for_each(strip_bounds);
    }
}

/// Qwen3 같은 모델이 내보내는 <think>…</think> 생각 과정을 지운다.
fn strip_think(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") {
            Some(j) => rest = &rest[i + j + "</think>".len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// 오래된 대화를 지운다. 도구 호출/결과 쌍이 끊기지 않도록 사용자의 일반 메시지에서만 자른다.
fn trim(hist: &mut Vec<Value>) {
    let is_user_text = |m: &Value| {
        m["role"] == "user" && (m["content"].is_string() || m["parts"][0]["text"].is_string())
    };
    while hist.len() > MAX_HISTORY {
        hist.remove(0);
        while !hist.is_empty() && !is_user_text(&hist[0]) {
            hist.remove(0);
        }
    }
}

fn system_prompt(tb: &Toolbox) -> String {
    let now = chrono::Local::now().format("%Y-%m-%d (%a) %H:%M");
    format!(
        "너는 토니 스타크의 AI 비서 J.A.R.V.I.S.다. 사용자를 '주인님'이라 부르고, 침착하고 정중하며 \
살짝 위트 있는 영국 집사 같은 말투의 한국어로 답한다.\n\
답변은 음성으로도 읽히므로 보통 1~3문장으로 짧게, 이모지 없이 말하듯 쓴다. 숫자와 단위는 읽기 쉽게 풀어 쓴다.\n\
화면은 마크다운을 그린다. 수식은 LaTeX($…$, 블록은 $$…$$), 코드는 언어를 적은 ``` 코드 상자로 쓰고, \
표·목록·굵은 글씨는 사용자가 코드·수식·정리를 원할 때만 쓴다.\n\
할 일을 말하면 add_todo(마감일이 있으면 YYYY-MM-DD), 끝냈다고 하면 complete_todo, 목록을 물으면 list_todos를 쓴다.\n\
날씨·기온·비 소식을 물으면 get_weather를 쓰고, 결과를 한두 문장으로 요약한다.\n\
너는 화면에 스스로 UI를 만들 수 있다. 답이 표·비교·단계·일정·수치 정리처럼 구조가 있거나 한동안 보면서 쓸 내용이면 \
긴 말 대신 make_panel로 패널을 띄우고 말로는 한두 문장만 한다. 계산·단위 변환·그래프·연습 문제처럼 직접 만지면 좋은 것은 \
kind=html로 작은 도구를 만든다. 정말 도움이 될 때만 쓰고, 같은 주제면 같은 id로 고친다. 패널을 옮겨 달라면 arrange_panel.\n\
사용자의 말은 음성 인식 결과라 오타가 있을 수 있으니 의도를 추론하되, 애매하면 짧게 되묻는다.\n\
PC를 다룰 때는 반드시 도구를 쓰고, 도구가 실패하면 이유를 짧게 알린다. 할 수 없는 일은 솔직히 말한다.\n\
파일을 만들어 달라면 create_file로 문서/Jarvis 폴더에 만든다.\n\
파일을 올리겠다고 하면 open_file_picker, 예전에 올린 파일을 다시 보여 달라고 하면 show_file을 쓴다.\n\
사용자가 기억·기록·메모해 달라고 하거나 일정·마감·선호처럼 나중에 다시 쓸 정보를 말하면 remember로 기록한다. \
주제 태그 2~4개를 붙이고, 이미 있는 기록과 관련 있으면 related에 그 제목을 넣어 이어 준다. \
전에 말한 것이나 기록한 내용을 물으면 먼저 recall로 찾아보고 답한다.\n\
열 수 있는 앱: {apps}\n\
현재 시각: {now}\n\
현재 기록 제목: {titles}",
        apps = tb.app_names().join(", "),
        titles = {
            let t = tb.memory.titles(40);
            if t.is_empty() { "(없음)".to_string() } else { t.join(", ") }
        }
    )
}

#[cfg(test)]
mod tests {
    use super::{strip_bounds, strip_think};

    #[test]
    fn strips_think_tags() {
        assert_eq!(strip_think("<think>음…</think> 네, 주인님."), "네, 주인님.");
        assert_eq!(strip_think("안녕 <think>미완성"), "안녕");
    }
    use serde_json::json;

    #[test]
    fn strips_nested_bounds() {
        let mut v = json!({"type":"object","properties":{"seconds":{"type":"integer","minimum":1,"maximum":9}}});
        strip_bounds(&mut v);
        assert_eq!(
            v,
            json!({"type":"object","properties":{"seconds":{"type":"integer"}}})
        );
    }
}
