# 자비스 서버 (보안 모드)

API 키를 브라우저에 두지 않으려고 만든 작은 서버입니다. Cloudflare Workers 무료 요금제(하루 10만 요청)에서 돌아갑니다.

```
브라우저(GitHub Pages)  ──로그인 토큰──▶  자비스 서버(Workers)  ──API 키──▶  Groq
   화면 도구 실행                         시스템 지시 작성 · 입력 검사
   (타이머·할 일·패널…)                  서버 도구 실행 (웹 검색·페이지 읽기)
```

- **키는 서버 비밀 변수에만** 있습니다. 브라우저는 비밀 문구로 로그인해서 받은 **기한 있는 토큰**(기본 30일)만 가집니다.
- 토큰을 무효로 만들고 싶으면 `TOKEN_SECRET`만 바꾸면 됩니다 (모든 기기가 다시 로그인).
- `ALLOWED_ORIGINS`에 적힌 웹 주소에서 온 요청만 받습니다.

## 준비물 (비밀 값 3개)

| 이름 | 무엇 | 만드는 법 |
|---|---|---|
| `GROQ_API_KEY` | Groq 키 | [console.groq.com/keys](https://console.groq.com/keys) → Create API Key. **예전에 채팅에 붙여 넣었던 키는 삭제하고 새로 만드세요.** |
| `JARVIS_PASSPHRASE` | 로그인 비밀 문구 (12자 이상) | 남이 못 맞힐 문장. 예: `우리집 고양이는 화요일에 피아노를 친다` |
| `TOKEN_SECRET` | 토큰 서명용 무작위 값 (32자 이상) | 터미널에서 `node -e "console.log(crypto.randomBytes(32).toString('base64url'))"` |

## 방법 A. GitHub Actions로 배포 (추천 · 명령어 거의 없음)

1. [dash.cloudflare.com](https://dash.cloudflare.com) 가입 (무료, 카드 불필요).
2. **계정 ID**: 대시보드 오른쪽 아래 *Account ID* 복사.
3. **API 토큰**: My Profile → API Tokens → Create Token → *Edit Cloudflare Workers* 템플릿 → Create.
4. GitHub 저장소 → Settings → Secrets and variables → Actions → **New repository secret**으로 5개 추가:
   `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`, `GROQ_API_KEY`, `JARVIS_PASSPHRASE`, `TOKEN_SECRET`
5. Actions 탭 → **서버 배포 (Cloudflare Workers)** → Run workflow.
6. 끝나면 로그에 `https://jarvis-api.<내이름>.workers.dev` 주소가 나옵니다.
7. 자비스 웹 → SETTINGS → 두뇌 모델 **내 서버** → 서버 주소와 비밀 문구 → 로그인.

push할 때마다 자동으로 배포하려면 Variables 탭에 `DEPLOY_SERVER` = `true`를 추가하세요.

## 방법 B. Cloudflare 대시보드에 붙여 넣기

1. Workers & Pages → Create → *Start with Hello World* → 이름 `jarvis-api` → Deploy.
2. Edit code → 기존 코드를 지우고 [`server/dist/worker.bundle.js`](dist/worker.bundle.js) 내용을 통째로 붙여 넣기 → Deploy.
3. Settings → Variables and Secrets:
   - **Secret**: `GROQ_API_KEY`, `JARVIS_PASSPHRASE`, `TOKEN_SECRET`
   - **Text**: `ALLOWED_ORIGINS` = `https://bidulgi09.github.io`
4. 서버를 고칠 때마다 `node scripts/gen-shared.mjs`로 번들을 다시 만들어 붙여 넣어야 합니다.

## 방법 C. 명령어 (Codespaces 등)

```bash
cd server
npx wrangler@4 login            # Codespaces라면 대신 export CLOUDFLARE_API_TOKEN=...
npx wrangler@4 secret put GROQ_API_KEY
npx wrangler@4 secret put JARVIS_PASSPHRASE
npx wrangler@4 secret put TOKEN_SECRET
npx wrangler@4 deploy
```

## 내 컴퓨터에서 돌려 보기

```bash
cp server/.dev.vars.example server/.dev.vars   # 값 채우기 (git에 안 올라감)
node server/dev.mjs                              # http://127.0.0.1:8787
node --test server/test/*.test.mjs               # 보안·동작 테스트 (네트워크 없이)
```

화면을 `http://127.0.0.1:5500`에서 띄운다면 `.dev.vars`의 `ALLOWED_ORIGINS`에 그 주소를 넣으세요.

## 선택 설정 (wrangler.toml의 `[vars]` 또는 대시보드 Text 변수)

| 이름 | 기본값 | 설명 |
|---|---|---|
| `MODEL` / `MODEL_SMALL` | `openai/gpt-oss-120b` / `openai/gpt-oss-20b` | 대화 모델과, 한도·바쁨일 때 대신 쓸 모델 |
| `SEARCH_MODEL` | `openai/gpt-oss-120b` | 웹 검색(browser_search)에 쓸 모델 |
| `VISION_MODEL` | `qwen/qwen3.8-27b` | 이미지 설명 |
| `STT_MODEL` | `whisper-large-v3-turbo` | 받아쓰기 |
| `TOKEN_DAYS` | `30` | 로그인 유지 기간 (1~90일) |
| `TZ` | `Asia/Seoul` | 검색할 때 '오늘'의 기준 |

## API

| 경로 | 인증 | 설명 |
|---|---|---|
| `GET /api/health` | 없음 | 살아 있는지, 설정이 끝났는지 |
| `POST /api/login` | 없음 | `{passphrase}` → `{token, exp}` (IP당 10분에 6번) |
| `GET /api/status` | 토큰 | 모델·기능 |
| `POST /api/chat` | 토큰 | `{messages, tools, context}` → `{done, reply}` 또는 `{done:false, calls}` (화면 도구 실행 요청) |
| `POST /api/stt` | 토큰 | 소리 파일(본문) → `{text}` |
| `POST /api/vision` | 토큰 | `{image: dataURL, question}` → `{text}` |
| `POST /api/search` | 토큰 | `{query}` → `{answer, sources}` |
