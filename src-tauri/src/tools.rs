//! 자비스가 PC에서 실행할 수 있는 도구들.
//! 프로그램 실행은 apps.json에 등록된 것만 허용한다 (모델이 임의 명령을 못 돌리게).
//! 파일은 문서/Jarvis 폴더 안에만 만든다 (다른 곳의 파일을 덮어쓰지 못하게).

use crate::memory::Memory;
use crate::todo::Todos;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub struct Toolbox {
    apps: BTreeMap<String, String>,
    files_dir: PathBuf,
    pub memory: Memory,
    pub todos: Todos,
    http: reqwest::Client,
    /// 날씨 기본 지역 (화면 SETTINGS에서 바꾸면 settings.json에 저장)
    city: Mutex<String>,
}

/// 문서/Jarvis (윈도우: %USERPROFILE%\Documents\Jarvis, 맥·리눅스: ~/Documents/Jarvis)
pub fn jarvis_dir() -> PathBuf {
    let home = std::env::var_os(if cfg!(target_os = "windows") {
        "USERPROFILE"
    } else {
        "HOME"
    })
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Jarvis")
}

impl Toolbox {
    /// apps.json: { "크롬": "chrome", "메모장": "notepad", ... }
    pub fn load(path: &Path) -> Self {
        let apps = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(&s).ok())
            .unwrap_or_else(default_apps);
        let dir = jarvis_dir();
        let city = std::env::var("JARVIS_CITY")
            .ok()
            .filter(|c| !c.trim().is_empty())
            .or_else(|| {
                std::fs::read_to_string(dir.join("settings.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                    .and_then(|v| v["city"].as_str().map(String::from))
            })
            .unwrap_or_default();
        Self {
            apps,
            memory: Memory::load(dir.join("memory.json")),
            todos: Todos::load(dir.join("todos.json")),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_default(),
            city: Mutex::new(city),
            files_dir: dir,
        }
    }

    pub fn set_city(&self, city: &str) -> Result<()> {
        let city = city.trim().chars().take(40).collect::<String>();
        if let Ok(mut c) = self.city.lock() {
            *c = city.clone();
        }
        std::fs::create_dir_all(&self.files_dir)?;
        std::fs::write(
            self.files_dir.join("settings.json"),
            serde_json::to_vec_pretty(&json!({ "city": city }))?,
        )
        .context("설정을 저장하지 못했습니다")
    }

    pub fn app_names(&self) -> Vec<&str> {
        self.apps.keys().map(String::as_str).collect()
    }
}

fn default_apps() -> BTreeMap<String, String> {
    let pairs: &[(&str, &str)] = if cfg!(target_os = "windows") {
        &[
            ("메모장", "notepad"),
            ("계산기", "calc"),
            ("탐색기", "explorer"),
            ("크롬", "chrome"),
            ("VS Code", "code"),
        ]
    } else if cfg!(target_os = "macos") {
        &[
            ("메모", "Notes"),
            ("계산기", "Calculator"),
            ("파인더", "Finder"),
            ("크롬", "Google Chrome"),
            ("VS Code", "Visual Studio Code"),
        ]
    } else {
        &[
            ("터미널", "x-terminal-emulator"),
            ("파일", "nautilus"),
            ("크롬", "google-chrome"),
            ("VS Code", "code"),
        ]
    };
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn schemas(tb: &Toolbox) -> Value {
    json!([
        {
            "name": "get_datetime",
            "description": "현재 날짜, 요일, 시각을 돌려준다.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "open_app",
            "description": "등록된 PC 프로그램을 실행한다. name은 반드시 목록 중 하나여야 한다.",
            "input_schema": {
                "type": "object",
                "properties": { "name": { "type": "string", "enum": tb.app_names() } },
                "required": ["name"]
            }
        },
        {
            "name": "open_url",
            "description": "기본 브라우저로 https 주소를 연다. 검색은 검색 결과 페이지 URL을 만들어 연다.",
            "input_schema": {
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }
        },
        {
            "name": "set_volume",
            "description": "시스템 볼륨을 0~100 퍼센트로 맞춘다.",
            "input_schema": {
                "type": "object",
                "properties": { "percent": { "type": "integer", "minimum": 0, "maximum": 100 } },
                "required": ["percent"]
            }
        },
        {
            "name": "set_timer",
            "description": "초 단위 타이머를 건다. 끝나면 홀로그램이 알리고 자비스가 말한다.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "seconds": { "type": "integer", "minimum": 1, "maximum": 86400 },
                    "label": { "type": "string" }
                },
                "required": ["seconds"]
            }
        },
        {
            "name": "create_file",
            "description": "텍스트 파일을 문서/Jarvis 폴더에 만들고 연다. 메모, 할 일 목록, 표(csv), 글 초안 등. filename에 확장자(txt, md, csv, json, html)를 붙이고 content에 전체 내용을 넣는다.",
            "input_schema": {
                "type": "object",
                "properties": { "filename": { "type": "string" }, "content": { "type": "string" } },
                "required": ["filename", "content"]
            }
        },
        {
            "name": "remember",
            "description": "정보를 기록 네트워크에 저장한다. 같은 제목이 있으면 내용을 덧붙인다. tags는 주제 2~4개, related는 이어질 기존 기록 제목.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "content": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["title", "content"]
            }
        },
        {
            "name": "recall",
            "description": "기록에서 찾는다. query가 비면 최근 기록을 돌려준다.",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } } }
        },
        {
            "name": "open_file_picker",
            "description": "파일 선택 창을 연다. 사용자가 파일·사진·문서를 올리겠다, 첨부하겠다고 하면 호출한다.",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "show_file",
            "description": "예전에 첨부한 파일을 화면에 다시 띄운다. name은 파일 이름 일부나 종류(PDF, 사진, 워드)여도 된다. 비우면 가장 최근 파일.",
            "input_schema": { "type": "object", "properties": { "name": { "type": "string" } } }
        },
        {
            "name": "add_todo",
            "description": "할 일 목록에 추가한다. due는 마감일(YYYY-MM-DD)이 있을 때만.",
            "input_schema": {
                "type": "object",
                "properties": { "text": { "type": "string" }, "due": { "type": "string" } },
                "required": ["text"]
            }
        },
        {
            "name": "list_todos",
            "description": "할 일 목록을 돌려준다 (남은 것 먼저, 번호 포함).",
            "input_schema": { "type": "object", "properties": {} }
        },
        {
            "name": "complete_todo",
            "description": "할 일을 끝냄으로 표시한다. query는 할 일 내용 일부나 '2번' 같은 번호.",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        },
        {
            "name": "remove_todo",
            "description": "할 일을 목록에서 지운다. query는 내용 일부나 번호.",
            "input_schema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
        },
        {
            "name": "show_todos",
            "description": "할 일 카드를 화면에 열거나 닫는다.",
            "input_schema": { "type": "object", "properties": { "open": { "type": "boolean" } }, "required": ["open"] }
        },
        {
            "name": "get_weather",
            "description": "현재 날씨와 4일 예보를 가져와 화면에 날씨 카드를 띄운다. place는 지역 이름(예: 대구, 서울 강남구). 사용자가 지역을 말하지 않았으면 비운다.",
            "input_schema": { "type": "object", "properties": { "place": { "type": "string" } } }
        },
        {
            "name": "show_memory",
            "description": "기록 네트워크 화면을 열거나 닫는다.",
            "input_schema": {
                "type": "object",
                "properties": { "open": { "type": "boolean" } },
                "required": ["open"]
            }
        },
        {
            "name": "set_hologram",
            "description": "홀로그램 분위기를 바꾼다. calm(기본 청록), focus(맑은 백청색, 집중), nebula(보라·자홍 성운), alert(호박색 경계).",
            "input_schema": {
                "type": "object",
                "properties": { "mood": { "type": "string", "enum": ["calm", "focus", "nebula", "alert"] } },
                "required": ["mood"]
            }
        }
    ])
}

pub async fn run(app: &AppHandle, tb: &Toolbox, name: &str, input: &Value) -> Result<String> {
    let _ = app.emit("tool", json!({ "name": name, "input": input }));
    match name {
        "get_datetime" => Ok(chrono::Local::now()
            .format("%Y년 %m월 %d일 %A %H시 %M분")
            .to_string()),

        "open_app" => {
            let key = input["name"].as_str().context("name이 없습니다")?;
            let target = tb
                .apps
                .get(key)
                .with_context(|| format!("'{key}'는 등록되지 않은 프로그램입니다"))?;
            launch_app(target)?;
            Ok(format!("{key} 실행함"))
        }

        "open_url" => {
            let url = input["url"].as_str().context("url이 없습니다")?;
            if !url.starts_with("https://") || url.chars().any(|c| c.is_whitespace() || c == '"') {
                bail!("https로 시작하는 올바른 주소만 열 수 있습니다");
            }
            open_url(url)?;
            Ok(format!("{url} 열었음"))
        }

        "set_volume" => {
            let p = input["percent"]
                .as_i64()
                .context("percent가 없습니다")?
                .clamp(0, 100) as u32;
            set_volume(p)?;
            Ok(format!("볼륨 {p}%"))
        }

        "set_timer" => {
            let secs = input["seconds"]
                .as_u64()
                .context("seconds가 없습니다")?
                .clamp(1, 86_400);
            let label = input["label"]
                .as_str()
                .unwrap_or("타이머")
                .chars()
                .take(30)
                .collect::<String>();
            let app2 = app.clone();
            let id = chrono::Local::now().timestamp_millis();
            let _ = app.emit(
                "timer-set",
                json!({ "id": id, "label": label, "seconds": secs }),
            );
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(secs)).await;
                let _ = app2.emit("timer-done", json!({ "id": id, "label": label }));
            });
            Ok(format!("{secs}초 타이머 시작"))
        }

        "set_hologram" => {
            let mood = input["mood"].as_str().unwrap_or("calm");
            if !["calm", "focus", "nebula", "alert"].contains(&mood) {
                bail!("알 수 없는 분위기: {mood}");
            }
            let _ = app.emit("hologram", mood);
            Ok(format!("홀로그램 {mood}"))
        }

        "create_file" => {
            let raw = input["filename"].as_str().unwrap_or("jarvis.txt");
            let content = input["content"].as_str().unwrap_or_default();
            let path = create_file(&tb.files_dir, raw, content)?;
            let shown = path.display().to_string();
            if let Err(e) = open_path(&path) {
                return Ok(format!("{shown}에 만들었지만 열지는 못했음: {e:#}"));
            }
            Ok(format!("{shown}에 만들고 열었음"))
        }

        "remember" => {
            let list = |k: &str| -> Vec<String> {
                input[k]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let rec = tb.memory.upsert(
                input["title"].as_str().unwrap_or_default(),
                input["content"].as_str().unwrap_or_default(),
                &list("tags"),
                &list("related"),
            )?;
            let _ = app.emit("memory-changed", json!({ "id": rec.id }));
            Ok(format!("기록함: {}", rec.title))
        }

        "recall" => {
            let found = tb
                .memory
                .search(input["query"].as_str().unwrap_or_default(), 8);
            if found.is_empty() {
                return Ok("관련 기록 없음".into());
            }
            Ok(serde_json::to_string(&found)?)
        }

        // 파일 선택 창과 파일 뷰어는 화면(웹뷰) 쪽에서 처리한다
        "open_file_picker" => {
            let _ = app.emit("file-picker", ());
            Ok("화면에 파일 선택 창을 띄우도록 했음 (안 열리면 화면의 '파일 고르기' 버튼을 눌러 달라고 안내)".into())
        }

        "show_file" => {
            let name = input["name"].as_str().unwrap_or_default();
            let _ = app.emit("show-file", name);
            Ok(format!("화면에 '{name}' 파일을 띄우도록 했음"))
        }

        "show_memory" => {
            let open = input["open"].as_bool().unwrap_or(true);
            let _ = app.emit("memory-panel", open);
            Ok(if open {
                "기록 화면을 열었음"
            } else {
                "기록 화면을 닫았음"
            }
            .into())
        }

        "add_todo" => {
            let t = tb.todos.add(
                input["text"].as_str().unwrap_or_default(),
                input["due"].as_str().unwrap_or_default(),
            )?;
            let _ = app.emit("todos-changed", ());
            let _ = app.emit("todo-panel", json!({ "open": true, "auto": true }));
            Ok(if t.due.is_empty() {
                format!("추가함: {}", t.text)
            } else {
                format!("추가함: {} (마감 {})", t.text, t.due)
            })
        }

        "list_todos" => {
            let list = tb.todos.sorted();
            if list.is_empty() {
                return Ok("할 일 없음".into());
            }
            let rows: Vec<Value> = list
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let due = if t.due.is_empty() {
                        Value::Null
                    } else {
                        json!(t.due)
                    };
                    json!({ "n": i + 1, "text": t.text, "due": due, "done": t.done })
                })
                .collect();
            Ok(serde_json::to_string(&rows)?)
        }

        "complete_todo" | "remove_todo" => {
            let q = input["query"].as_str().unwrap_or_default();
            let t = tb.todos.find(q).context("그런 할 일이 없습니다")?;
            if name == "complete_todo" {
                tb.todos.toggle(&t.id, Some(true))?;
            } else {
                tb.todos.remove(&t.id)?;
            }
            let _ = app.emit("todos-changed", ());
            Ok(format!(
                "{}: {}",
                if name == "complete_todo" {
                    "완료"
                } else {
                    "지움"
                },
                t.text
            ))
        }

        "show_todos" => {
            let open = input["open"].as_bool().unwrap_or(true);
            let _ = app.emit("todo-panel", json!({ "open": open }));
            Ok(if open {
                "할 일 카드를 열었음"
            } else {
                "할 일 카드를 닫았음"
            }
            .into())
        }

        "get_weather" => {
            let asked = input["place"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string();
            let place = if asked.is_empty() {
                tb.city.lock().map(|c| c.clone()).unwrap_or_default()
            } else {
                asked
            };
            if place.is_empty() {
                bail!("지역을 모릅니다. 지역 이름을 말해 달라고 하거나, 화면 SETTINGS의 날씨 기본 지역을 채워 달라고 안내");
            }
            let w = crate::weather::get(&tb.http, &place).await?;
            let _ = app.emit("weather", w.clone());
            Ok(w.to_string())
        }

        other => bail!("알 수 없는 도구: {other}"),
    }
}

// ---------- 파일 만들기 ----------

const FILE_EXTS: [&str; 5] = ["txt", "md", "csv", "json", "html"];

/// 이름에서 폴더 경로·금지 문자를 걷어내고, 같은 이름이 있으면 "이름 (2).txt"처럼 피해서 만든다.
pub fn create_file(dir: &Path, raw_name: &str, content: &str) -> Result<PathBuf> {
    let base = Path::new(raw_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("jarvis.txt");
    let mut name: String = base
        .chars()
        .map(|c| {
            if "\\/:*?\"<>|".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .chars()
        .take(80)
        .collect();
    if name.is_empty() {
        name = "jarvis.txt".into();
    }
    let ext_ok = Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| FILE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false);
    if !ext_ok {
        name.push_str(".txt");
    }
    std::fs::create_dir_all(dir).context("문서/Jarvis 폴더를 만들지 못했습니다")?;
    let stem = Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("jarvis")
        .to_string();
    let ext = Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("txt")
        .to_string();
    let mut path = dir.join(&name);
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    // 정규화 후에도 폴더 밖이면 거절 (이중 안전장치)
    if path.parent() != Some(dir) {
        bail!("문서/Jarvis 폴더 밖에는 파일을 만들 수 없습니다");
    }
    std::fs::write(&path, content).context("파일을 쓰지 못했습니다")?;
    Ok(path)
}

fn open_path(path: &Path) -> Result<()> {
    let mut cmd;
    if cfg!(target_os = "windows") {
        cmd = Command::new("cmd");
        cmd.arg("/C").arg("start").arg("").arg(path);
    } else if cfg!(target_os = "macos") {
        cmd = Command::new("open");
        cmd.arg(path);
    } else {
        cmd = Command::new("xdg-open");
        cmd.arg(path);
    }
    quiet(cmd).spawn().context("파일을 열지 못했습니다")?;
    Ok(())
}

// ---------- 플랫폼별 실행 ----------

fn quiet(mut cmd: Command) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn launch_app(target: &str) -> Result<()> {
    let mut cmd;
    if cfg!(target_os = "windows") {
        cmd = Command::new("cmd");
        cmd.args(["/C", "start", "", target]);
    } else if cfg!(target_os = "macos") {
        cmd = Command::new("open");
        cmd.args(["-a", target]);
    } else {
        cmd = Command::new(target);
    }
    quiet(cmd)
        .spawn()
        .with_context(|| format!("{target} 실행 실패"))?;
    Ok(())
}

fn open_url(url: &str) -> Result<()> {
    let mut cmd;
    if cfg!(target_os = "windows") {
        // cmd start는 &를 명령 구분자로 읽으므로 rundll32로 연다
        cmd = Command::new("rundll32");
        cmd.args(["url.dll,FileProtocolHandler", url]);
    } else if cfg!(target_os = "macos") {
        cmd = Command::new("open");
        cmd.arg(url);
    } else {
        cmd = Command::new("xdg-open");
        cmd.arg(url);
    }
    quiet(cmd).spawn().context("브라우저를 열지 못했습니다")?;
    Ok(())
}

fn set_volume(percent: u32) -> Result<()> {
    let status = if cfg!(target_os = "windows") {
        // 윈도우는 기본 명령이 없어 NirCmd(무료)를 쓴다: https://www.nirsoft.net/utils/nircmd.html
        let v = (percent as f64 / 100.0 * 65535.0).round() as u32;
        quiet(Command::new("nircmd"))
            .args(["setsysvolume", &v.to_string()])
            .status()
            .context("nircmd가 없습니다. NirCmd를 설치하고 PATH에 추가해 주세요")?
    } else if cfg!(target_os = "macos") {
        Command::new("osascript")
            .args(["-e", &format!("set volume output volume {percent}")])
            .status()?
    } else {
        Command::new("pactl")
            .args(["set-sink-volume", "@DEFAULT_SINK@", &format!("{percent}%")])
            .status()
            .context("pactl이 없습니다")?
    };
    if !status.success() {
        bail!("볼륨 변경 명령이 실패했습니다");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::create_file;

    #[test]
    fn create_file_stays_inside_folder() {
        let dir = std::env::temp_dir().join(format!("jarvis-test-{}", std::process::id()));
        let p = create_file(&dir, "../../evil/할 일.md", "내용").unwrap();
        assert_eq!(p.parent().unwrap(), dir);
        assert_eq!(p.file_name().unwrap().to_str().unwrap(), "할 일.md");
        let p2 = create_file(&dir, "할 일.md", "다시").unwrap();
        assert_eq!(p2.file_name().unwrap().to_str().unwrap(), "할 일 (2).md");
        let p3 = create_file(&dir, "run.exe", "x").unwrap();
        assert_eq!(p3.file_name().unwrap().to_str().unwrap(), "run.exe.txt");
        std::fs::remove_dir_all(&dir).ok();
    }
}
