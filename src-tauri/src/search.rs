//! 실시간 검색: Groq gpt-oss의 browser_search (GROQ_API_KEY), 안 되면 위키백과.
//! 결과 모양은 화면(웹 버전)과 같다: {via, answer, sources:[{title,url}], note?}

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

async fn groq(http: &reqwest::Client, key: &str, q: &str) -> Result<Value> {
    let today = chrono::Local::now().format("%Y년 %-m월 %-d일").to_string();
    let mut last = anyhow::anyhow!("검색 실패");
    for model in ["openai/gpt-oss-120b", "openai/gpt-oss-20b"] {
        let body = json!({
            "model": model, "reasoning_effort": "low", "max_completion_tokens": 1400, "temperature": 0.2,
            "tools": [{ "type": "browser_search" }], "tool_choice": "required",
            "messages": [
                { "role": "system", "content": format!("오늘은 {today}이다. 웹을 검색해서 질문에 답할 사실을 한국어로 5줄 이내로 정리한다. 날짜·숫자·이름은 정확히, 확실하지 않으면 그렇다고 쓴다. 마지막 줄에 '출처:' 다음에 실제로 본 페이지 URL을 1~3개 쉼표로 적는다.") },
                { "role": "user", "content": q }
            ]
        });
        let r = match http
            .post("https://api.groq.com/openai/v1/chat/completions")
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last = anyhow::anyhow!("검색 서버에 연결하지 못했습니다: {e}");
                continue;
            }
        };
        let status = r.status();
        let v: Value = r.json().await.unwrap_or_default();
        if !status.is_success() {
            last = anyhow::anyhow!(
                "검색 실패: {}",
                v["error"]["message"].as_str().unwrap_or(status.as_str())
            );
            if status.as_u16() == 401 {
                break;
            }
            continue;
        }
        let m = &v["choices"][0]["message"];
        let answer = m["content"].as_str().unwrap_or("").trim().to_string();
        if answer.is_empty() {
            last = anyhow::anyhow!("검색 결과가 비었습니다");
            continue;
        }
        let mut sources: Vec<Value> = vec![];
        let mut add = |url: &str, title: &str| {
            let url = url.trim_end_matches(['.', ',', ')']);
            if url.starts_with("http")
                && !sources.iter().any(|s| s["url"] == url)
                && sources.len() < 5
            {
                let host = url.split('/').nth(2).unwrap_or(url);
                sources.push(
                    json!({ "title": if title.is_empty() { host } else { title }, "url": url }),
                );
            }
        };
        if let Some(tools) = m["executed_tools"].as_array() {
            for t in tools {
                let rs = t["search_results"]["results"]
                    .as_array()
                    .or_else(|| t["search_results"].as_array());
                for x in rs.into_iter().flatten() {
                    add(
                        x["url"].as_str().unwrap_or(""),
                        x["title"].as_str().unwrap_or(""),
                    );
                }
            }
        }
        for w in answer.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')') {
            if w.starts_with("http") {
                add(w, "");
            }
        }
        let clean = match answer.rfind("출처") {
            Some(i) if answer[i..].contains(':') || answer[i..].contains('：') => {
                answer[..i].trim().to_string()
            }
            _ => answer.clone(),
        };
        return Ok(json!({ "via": "web", "answer": clean, "sources": sources }));
    }
    Err(last)
}

async fn wiki(http: &reqwest::Client, q: &str) -> Result<Value> {
    let v: Value = http
        .get("https://ko.wikipedia.org/w/api.php")
        .query(&[
            ("action", "query"),
            ("generator", "search"),
            ("gsrsearch", q),
            ("gsrlimit", "3"),
            ("prop", "extracts"),
            ("exintro", "1"),
            ("explaintext", "1"),
            ("exchars", "700"),
            ("format", "json"),
        ])
        .header("User-Agent", "JARVIS-desktop/1.0")
        .send()
        .await
        .context("검색 서버에 연결하지 못했습니다")?
        .json()
        .await
        .unwrap_or_default();
    let mut pages: Vec<Value> = v["query"]["pages"]
        .as_object()
        .map(|o| o.values().cloned().collect())
        .unwrap_or_default();
    if pages.is_empty() {
        bail!("'{q}'에 대한 검색 결과가 없습니다");
    }
    pages.sort_by_key(|p| p["index"].as_i64().unwrap_or(0));
    let answer = pages
        .iter()
        .map(|p| {
            format!(
                "{}: {}",
                p["title"].as_str().unwrap_or(""),
                p["extract"]
                    .as_str()
                    .unwrap_or("")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let sources: Vec<Value> = pages
        .iter()
        .map(|p| {
            let t = p["title"].as_str().unwrap_or("");
            json!({ "title": format!("{t} - 위키백과"), "url": format!("https://ko.wikipedia.org/wiki/{}", t.replace(' ', "_")) })
        })
        .collect();
    Ok(
        json!({ "via": "wikipedia", "note": "위키백과 결과라 최신 소식이 아닐 수 있음", "answer": answer, "sources": sources }),
    )
}

pub async fn search(http: &reqwest::Client, q: &str) -> Result<Value> {
    let q: String = q.trim().chars().take(300).collect();
    if q.is_empty() {
        bail!("검색어가 비었습니다");
    }
    let key = std::env::var("GROQ_API_KEY").unwrap_or_default();
    let mut first = None;
    if !key.trim().is_empty() {
        match groq(http, key.trim(), &q).await {
            Ok(v) => return Ok(v),
            Err(e) => first = Some(e),
        }
    }
    match wiki(http, &q).await {
        Ok(mut v) => {
            if let Some(e) = first {
                v["note"] = json!(format!(
                    "웹 검색이 안 되어({e}) 위키백과로 찾음, 최신이 아닐 수 있음"
                ));
            }
            Ok(v)
        }
        Err(e) => Err(first.unwrap_or(e)),
    }
}
