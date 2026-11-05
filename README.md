# J.A.R.V.I.S.

아이언맨 스타일 AI 비서 웹 앱.

## 현재 상태 (MK.I)

`web/index.html` 한 파일로 된 HUD 웹 앱입니다.

- 아크 리액터 코어 애니메이션 (대기 / 생각 중 / 말하는 중 / 듣는 중 상태 표시)
- Claude를 두뇌로 쓰는 대화 (claude.ai 아티팩트의 `sample` 기능 사용, API 키 불필요)
- 음성 출력: 브라우저 내장 TTS (`speechSynthesis`, ko-KR)
- 음성 입력: Web Speech API 시도, 막힌 환경에서는 텍스트 입력으로 대체
- 자비스가 직접 쓰는 도구 (tool use)
  - `get_datetime` 현재 시각
  - `set_timer` HUD 타이머 + 알람
  - `add_note` / `list_notes` 메모 로그 (브라우저 저장)
  - `set_hud_color` HUD 색상 프로토콜 (cyan / red / gold / green)
  - `show_link` 웹사이트 링크 띄우기

> 두뇌 연결(`window.claude.use("sample")`)은 claude.ai에서 아티팩트로 열었을 때만 동작합니다.

## 로드맵

- [x] MK.I — HUD + 대화 + 음성 출력 + 페이지 내부 도구
- [ ] MK.II — 로컬 서버 (Python FastAPI) + Claude API 직접 호출, 마이크 음성 인식 정식 지원
- [ ] MK.III — PC 제어 (프로그램 실행, 볼륨, 파일 열기) 도구 추가
- [ ] MK.IV — 정보 알림 (날씨, 일정, 뉴스 브리핑)
- [ ] MK.V — 호출어 "자비스" 상시 대기
