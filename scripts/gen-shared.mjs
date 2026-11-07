// ui/shared의 도구 표·시스템 지시·말투를 서버 코드로 옮기고, 대시보드에 붙여 넣을 한 파일짜리 번들을 만든다.
//   node scripts/gen-shared.mjs          → server/src/shared.gen.js, server/dist/worker.bundle.js 생성
//   node scripts/gen-shared.mjs --check  → 생성물이 최신인지만 확인 (CI용)
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (p) => readFileSync(join(root, p), "utf8");
const tools = JSON.parse(read("ui/shared/tools.json"));
const personas = JSON.parse(read("ui/shared/personas.json"));
const prompt = read("ui/shared/prompt.txt");

for (const t of tools) {
  if (!/^[a-z_]{2,40}$/.test(t.name)) throw new Error("도구 이름이 이상합니다: " + t.name);
  if (t.parameters?.type !== "object") throw new Error(t.name + ": parameters.type은 object여야 합니다");
  if (![null, "client", "server"].includes(t.web) || ![null, "native", "ui"].includes(t.desktop)) throw new Error(t.name + ": web/desktop 값이 이상합니다");
}

const gen = `// 자동 생성 파일 — 직접 고치지 말고 ui/shared/*를 고친 뒤 node scripts/gen-shared.mjs
export const TOOLS = ${JSON.stringify(tools)};
export const PERSONAS = ${JSON.stringify(personas)};
export const PROMPT = ${JSON.stringify(prompt)};
`;

// 한 파일 번들: 우리 모듈끼리만 import하므로, import 줄을 지우고 export 키워드를 떼어 이어 붙이면 된다
const strip = (src) => src.replace(/^import[\s\S]*?from\s+"\.\/[\w.]+";\n/gm, "").replace(/^export (const|function|async function|class) /gm, "$1 ");
const lib = read("server/src/lib.js"), worker = read("server/src/worker.js");
const bundle = `// J.A.R.V.I.S. 서버 — 한 파일 번들 (Cloudflare 대시보드 편집기에 그대로 붙여 넣기용)
// 자동 생성: node scripts/gen-shared.mjs  · 원본: server/src/*.js, ui/shared/*
${strip(gen)}
${strip(lib)}
${worker.replace(/^import[\s\S]*?from\s+"\.\/[\w.]+";\n/gm, "")}`;

const outs = { "server/src/shared.gen.js": gen, "server/dist/worker.bundle.js": bundle };
if (process.argv.includes("--check")) {
  const stale = Object.entries(outs).filter(([p, v]) => !existsSync(join(root, p)) || read(p) !== v).map(([p]) => p);
  if (stale.length) { console.error("최신이 아닙니다. node scripts/gen-shared.mjs 를 실행하세요:", stale.join(", ")); process.exit(1); }
  console.log("생성물이 최신입니다.");
} else {
  for (const [p, v] of Object.entries(outs)) { mkdirSync(dirname(join(root, p)), { recursive: true }); writeFileSync(join(root, p), v); }
  console.log("생성함:", Object.keys(outs).join(", "));
}
