// 내 컴퓨터·Codespaces에서 서버를 돌려 보는 용도 (Cloudflare 없이). Node 20 이상.
//   cp .dev.vars.example .dev.vars   (값 채우기)
//   node dev.mjs                      → http://127.0.0.1:8787
import { createServer } from "node:http";
import { readFileSync, existsSync } from "node:fs";
import worker from "./src/worker.js";

export function loadVars(path = new URL("./.dev.vars", import.meta.url)) {
  const env = {};
  if (existsSync(path)) for (const line of readFileSync(path, "utf8").split(/\r?\n/)) {
    const m = /^\s*([A-Z_][A-Z0-9_]*)\s*=\s*(.*?)\s*$/.exec(line);
    if (m && !line.trim().startsWith("#")) env[m[1]] = m[2].replace(/^"(.*)"$/, "$1");
  }
  for (const k of Object.keys(process.env)) if (/^(GROQ_|JARVIS_|TOKEN_|ALLOWED_ORIGINS|MODEL|SEARCH_MODEL|VISION_MODEL|STT_MODEL|TZ$)/.test(k)) env[k] ??= process.env[k];
  return env;
}

export function serve(env, port = 8787, host = "127.0.0.1") {
  const server = createServer(async (req, res) => {
    const chunks = [];
    for await (const c of req) chunks.push(c);
    const body = chunks.length ? Buffer.concat(chunks) : undefined;
    const headers = new Headers();
    for (const [k, v] of Object.entries(req.headers)) if (v != null) headers.set(k, Array.isArray(v) ? v.join(", ") : v);
    headers.set("x-real-ip", req.socket.remoteAddress || "local");
    const request = new Request(`http://${req.headers.host}${req.url}`, { method: req.method, headers, body: ["GET", "HEAD"].includes(req.method) ? undefined : body });
    const r = await worker.fetch(request, env);
    res.writeHead(r.status, Object.fromEntries(r.headers));
    res.end(Buffer.from(await r.arrayBuffer()));
  });
  return new Promise((ok) => server.listen(port, host, () => ok(server)));
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const env = loadVars();
  const s = await serve(env, Number(process.env.PORT) || 8787);
  console.log(`J.A.R.V.I.S. 서버: http://127.0.0.1:${s.address().port}  (허용 출처: ${env.ALLOWED_ORIGINS || "없음"})`);
}
