# J.A.R.V.I.S.

아이언맨 자비스를 목표로 하는 데스크톱 AI 비서. Rust(Tauri) + 로컬 Whisper 음성인식 + Claude.

![MK.II 홀로그램](docs/hologram.png)

## 구조 (MK.II)

```
[마이크] ─cpal─▶ 녹음(Rust) ─16kHz mono─▶ whisper.cpp (로컬, whisper-rs)
                                               │ 받아적은 문장
                                               ▼
[홀로그램 UI] ◀─이벤트─ Tauri ◀─────── Claude API + 도구 루프 (Rust)
  WebGL2 셰이더          │                     │
  웹뷰 TTS로 말하기      └─ mic-level, timer,   └─ 프로그램 실행, 브라우저,
                            hologram 이벤트        볼륨, 타이머, 홀로그램 분위기
```

| 파일 | 역할 |
|---|---|
| `src-tauri/src/main.rs` | 앱 시작, `.env` 로드, 프런트와 연결되는 명령 (`start_listening`, `stop_listening`, `ask`, `status`, `reset`) |
| `src-tauri/src/audio.rs` | 누르고 있는 동안 녹음, 16kHz 모노 변환, 40ms마다 볼륨을 홀로그램에 전달 |
| `src-tauri/src/stt.rs` | Whisper 모델을 한 번만 올려 재사용, 한국어 받아쓰기, 무음 환각 문구 필터 |
| `src-tauri/src/brain.rs` | Claude Messages API, 도구 호출 루프 (최대 6회), 대화 기록 관리 |
| `src-tauri/src/tools.rs` | `open_app`(apps.json 화이트리스트만), `open_url`, `set_volume`, `set_timer`, `set_hologram`, `get_datetime` |
| `ui/index.html` | 우주 홀로그램 (성운 fbm 셰이더, 중력 렌즈, 홀로그램 구체, 궤도 링, 입자 원반 2,600개) |

### 홀로그램이 부드러운 이유

모든 값(에너지, 마이크 볼륨, 색상, 크기)이 목표값으로 바로 바뀌지 않고
$x \leftarrow x + (x_{target} - x)(1 - e^{-k\,\Delta t})$ 로 매 프레임 미끄러집니다.
$\Delta t$를 쓰기 때문에 60Hz든 144Hz든 같은 속도로 움직이고, 입자는 케플러 법칙처럼
안쪽일수록 빠르게 ($\omega \propto r^{-3/2}$) 돕니다.

## 실행 방법 (Windows 기준)

### 1. 준비물

- [Rust](https://rustup.rs)
- [Tauri 사전 준비](https://v2.tauri.app/start/prerequisites/): Microsoft C++ Build Tools, WebView2
- **CMake**와 **LLVM(clang)**: whisper.cpp를 직접 컴파일하기 때문에 필요합니다
  ```
  winget install Kitware.CMake LLVM.LLVM
  ```
- Tauri CLI: `cargo install tauri-cli --version "^2"`

### 2. Whisper 모델 받기

[ggml 모델](https://huggingface.co/ggerganov/whisper.cpp/tree/main)을 `models/` 폴더에 넣습니다.

| 모델 | 크기 | 한국어 품질 | 추천 |
|---|---|---|---|
| `ggml-base.bin` | 142MB | 약함 | 테스트용 |
| `ggml-small.bin` | 466MB | 괜찮음 | **기본값** |
| `ggml-large-v3-turbo.bin` | 1.6GB | 좋음 | NVIDIA GPU가 있으면 |

```powershell
curl -L -o models/ggml-small.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

### 3. 설정

```powershell
copy .env.example .env
```
`.env`에 `ANTHROPIC_API_KEY`를 넣고, 열고 싶은 프로그램은 `apps.json`에 추가합니다.
볼륨 조절은 [NirCmd](https://www.nirsoft.net/utils/nircmd.html)를 PATH에 넣어야 동작합니다.

### 4. 실행

```powershell
cd src-tauri
cargo tauri dev                    # CPU
cargo tauri dev --features cuda    # NVIDIA GPU로 Whisper 가속
```

처음 말할 때 Whisper 모델을 메모리에 올리느라 몇 초 걸리고, 그 다음부터는 빠릅니다.

### 사용법

- **Space를 누르고 있는 동안 말하기**, 떼면 받아적고 대답
- 마이크 버튼을 길게 눌러도 됨
- 예: "크롬 열어줘", "유튜브에서 로파이 찾아줘", "라면 타이머 3분", "성운 모드로 바꿔줘", "볼륨 30으로"

홀로그램만 보고 싶으면 `ui/index.html`을 브라우저로 열면 데모 모드로 돌아갑니다.

## 로드맵

- [x] MK.I: 웹 HUD + 대화 + 음성 출력 (`legacy/`)
- [x] MK.II: Rust/Tauri, 로컬 Whisper, PC 제어 도구, 우주 홀로그램
- [ ] MK.III: 호출어 "자비스" 상시 대기 (VAD + 짧은 Whisper 확인)
- [ ] MK.IV: 답변 스트리밍 + 문장 단위로 바로 말하기 (체감 지연 줄이기)
- [ ] MK.V: 일정·날씨 브리핑, 로컬 TTS (자비스다운 목소리)
