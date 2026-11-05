//! 기록 네트워크 저장소: 문서/Jarvis/memory.json
//! 웹 버전의 localStorage 기록과 같은 형식이라 화면(ui/index.html)이 그대로 그린다.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub related: Vec<String>,
    pub created: i64,
    pub updated: i64,
}

pub struct Memory {
    path: PathBuf,
    records: Mutex<Vec<Record>>,
}

fn norm(t: &str) -> String {
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn clean_list(xs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in xs {
        let x = x.trim();
        if !x.is_empty() && !out.iter().any(|o| o == x) {
            out.push(x.to_string());
        }
    }
    out.truncate(8);
    out
}

impl Memory {
    pub fn load(path: PathBuf) -> Self {
        let records = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Record>>(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            records: Mutex::new(records),
        }
    }

    fn save(&self, recs: &[Record]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).context("기록 폴더를 만들지 못했습니다")?;
        }
        // 쓰는 도중 꺼져도 기존 파일이 깨지지 않게 임시 파일에 쓰고 바꿔치기
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(recs)?)
            .context("기록을 저장하지 못했습니다")?;
        std::fs::rename(&tmp, &self.path).context("기록 파일을 바꾸지 못했습니다")?;
        Ok(())
    }

    pub fn all(&self) -> Vec<Record> {
        self.records.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn titles(&self, n: usize) -> Vec<String> {
        let mut v = self.all();
        v.sort_by(|a, b| b.updated.cmp(&a.updated));
        v.into_iter().take(n).map(|r| r.title).collect()
    }

    pub fn upsert(
        &self,
        title: &str,
        content: &str,
        tags: &[String],
        related: &[String],
    ) -> Result<Record> {
        let title: String = title.trim().chars().take(60).collect();
        let content: String = content.trim().chars().take(4000).collect();
        if title.is_empty() || content.is_empty() {
            bail!("title과 content가 필요합니다");
        }
        let now = chrono::Local::now().timestamp_millis();
        let mut recs = self
            .records
            .lock()
            .map_err(|_| anyhow::anyhow!("기록 잠금 실패"))?;
        let key = norm(&title);
        let rec = if let Some(r) = recs.iter_mut().find(|r| norm(&r.title) == key) {
            if !r.content.contains(&content) {
                r.content = format!("{}\n{}", r.content, content);
            }
            let mut t = r.tags.clone();
            t.extend_from_slice(tags);
            r.tags = clean_list(&t);
            let mut l = r.related.clone();
            l.extend_from_slice(related);
            r.related = clean_list(&l);
            r.updated = now;
            r.clone()
        } else {
            let r = Record {
                id: format!("m{now:x}{:04x}", recs.len() as u32 & 0xffff),
                title,
                content,
                tags: clean_list(tags),
                related: clean_list(related),
                created: now,
                updated: now,
            };
            recs.push(r.clone());
            r
        };
        self.save(&recs)?;
        Ok(rec)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut recs = self
            .records
            .lock()
            .map_err(|_| anyhow::anyhow!("기록 잠금 실패"))?;
        recs.retain(|r| r.id != id);
        self.save(&recs)
    }

    pub fn search(&self, query: &str, n: usize) -> Vec<Record> {
        let mut all = self.all();
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        if words.is_empty() {
            all.sort_by(|a, b| b.updated.cmp(&a.updated));
            all.truncate(n);
            return all;
        }
        let mut scored: Vec<(usize, Record)> = all
            .into_iter()
            .filter_map(|r| {
                let title = r.title.to_lowercase();
                let hay = format!(
                    "{} {} {}",
                    title,
                    r.content.to_lowercase(),
                    r.tags.join(" ").to_lowercase()
                );
                let s: usize = words
                    .iter()
                    .map(|w| {
                        if title.contains(w.as_str()) {
                            3
                        } else if hay.contains(w.as_str()) {
                            1
                        } else {
                            0
                        }
                    })
                    .sum();
                (s > 0).then_some((s, r))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated.cmp(&a.1.updated)));
        scored.into_iter().take(n).map(|(_, r)| r).collect()
    }
}
