//! 자비스가 PC에서 실행할 수 있는 도구들.
//! 프로그램 실행은 apps.json에 등록된 것만 허용한다 (모델이 임의 명령을 못 돌리게).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub struct Toolbox {
    apps: BTreeMap<String, String>,
}

impl Toolbox {
    /// apps.json: { "크롬": "chrome", "메모장": "notepad", ... }
    pub fn load(path: &Path) -> Self {
        let apps = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(&s).ok())
            .unwrap_or_else(default_apps);
        Self { apps }
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

        other => bail!("알 수 없는 도구: {other}"),
    }
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
