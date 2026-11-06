//! 할 일 목록: 문서/Jarvis/todos.json
//! 웹 버전의 localStorage("jarvis.todos")와 같은 형식이라 화면이 그대로 그린다.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub due: String,
    #[serde(default)]
    pub done: bool,
    pub created: i64,
    #[serde(default, rename = "doneAt")]
    pub done_at: i64,
}

pub struct Todos {
    path: PathBuf,
    items: Mutex<Vec<Todo>>,
}

fn valid_due(d: &str) -> bool {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
}

impl Todos {
    pub fn load(path: PathBuf) -> Self {
        let items = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Todo>>(&s).ok())
            .unwrap_or_default();
        Self {
            path,
            items: Mutex::new(items),
        }
    }

    fn save(&self, items: &[Todo]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).context("할 일 폴더를 만들지 못했습니다")?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(items)?)
            .context("할 일을 저장하지 못했습니다")?;
        std::fs::rename(&tmp, &self.path).context("할 일을 저장하지 못했습니다")?;
        Ok(())
    }

    fn with<T>(&self, f: impl FnOnce(&mut Vec<Todo>) -> Result<T>) -> Result<T> {
        let mut g = self
            .items
            .lock()
            .map_err(|_| anyhow::anyhow!("할 일 잠금 실패"))?;
        let out = f(&mut g)?;
        self.save(&g)?;
        Ok(out)
    }

    /// 남은 것 먼저, 마감 빠른 순, 만든 순
    pub fn sorted(&self) -> Vec<Todo> {
        let mut v = self.items.lock().map(|g| g.clone()).unwrap_or_default();
        v.sort_by(|a, b| {
            let key = |t: &Todo| {
                (
                    t.done,
                    if t.due.is_empty() {
                        "9999".to_string()
                    } else {
                        t.due.clone()
                    },
                    t.created,
                )
            };
            key(a).cmp(&key(b))
        });
        v
    }

    pub fn add(&self, text: &str, due: &str) -> Result<Todo> {
        let text: String = text.trim().chars().take(200).collect();
        if text.is_empty() {
            bail!("할 일 내용이 비었습니다");
        }
        let now = chrono::Local::now().timestamp_millis();
        let t = Todo {
            id: format!("t{now:x}"),
            text,
            due: if valid_due(due.trim()) {
                due.trim().to_string()
            } else {
                String::new()
            },
            done: false,
            created: now,
            done_at: 0,
        };
        self.with(|v| {
            v.push(t.clone());
            Ok(())
        })?;
        Ok(t)
    }

    pub fn toggle(&self, id: &str, done: Option<bool>) -> Result<Option<Todo>> {
        self.with(|v| {
            Ok(v.iter_mut().find(|t| t.id == id).map(|t| {
                t.done = done.unwrap_or(!t.done);
                t.done_at = if t.done {
                    chrono::Local::now().timestamp_millis()
                } else {
                    0
                };
                t.clone()
            }))
        })
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        self.with(|v| {
            v.retain(|t| t.id != id);
            Ok(())
        })
    }

    pub fn clear_done(&self) -> Result<()> {
        self.with(|v| {
            v.retain(|t| !t.done);
            Ok(())
        })
    }

    /// "2번", "2" 같은 번호(정렬된 목록 기준) 또는 내용 일부로 찾는다
    pub fn find(&self, query: &str) -> Option<Todo> {
        let list = self.sorted();
        let q = query.trim();
        let digits: String = q.chars().filter(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty()
            && q.chars()
                .all(|c| c.is_ascii_digit() || c.is_whitespace() || "번째.".contains(c))
        {
            if let Ok(n) = digits.parse::<usize>() {
                return list.get(n.wrapping_sub(1)).cloned();
            }
        }
        let ql = q.to_lowercase();
        list.iter()
            .find(|t| !t.done && t.text.to_lowercase().contains(&ql))
            .or_else(|| list.iter().find(|t| t.text.to_lowercase().contains(&ql)))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_find_toggle() {
        let p = std::env::temp_dir().join(format!("jarvis-todo-{}.json", std::process::id()));
        let t = Todos::load(p.clone());
        t.add("우유 사기", "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        t.add("보고서 제출", "2026-01-02").unwrap();
        assert_eq!(t.sorted()[0].text, "보고서 제출");
        assert_eq!(t.find("2번").unwrap().text, "우유 사기");
        let id = t.find("우유").unwrap().id;
        t.toggle(&id, Some(true)).unwrap();
        assert!(Todos::load(p.clone()).sorted().last().unwrap().done);
        t.clear_done().unwrap();
        assert_eq!(t.sorted().len(), 1);
        let _ = std::fs::remove_file(p);
    }
}
