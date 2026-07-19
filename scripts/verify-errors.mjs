// Headless runtime verification for Plan 12 Phase 3 (structured IPC errors).
// Spins up the mock Vite build (no Rust), drives `useConnections.connect` with
// sentinel ids whose mock `invoke` rejects with a structured {kind, message}
// error — exactly as a migrated Tauri command does — and asserts the toast the
// UI renders is keyed off the error `kind`, not the message text. A legacy
// string error is checked too, to prove the fallback path still works.
// Exit code 0 = all checks passed.
import { spawn } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import puppeteer from "puppeteer-core";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "..");
const OUT =
  process.env.SHOT_DIR ||
  "C:/Users/Juan/AppData/Local/Temp/claude/C--Users-Juan-Documents-GitHub-Faro/ff56a5eb-30a3-4774-8488-185533f77982/scratchpad";
const PORT = 1425;
const URL = `http://localhost:${PORT}/`;

const CHROME_CANDIDATES = [
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
];
const chrome = CHROME_CANDIDATES.find((p) => existsSync(p));
if (!chrome) {
  console.error("No Chrome/Edge found.");
  process.exit(2);
}
const isWin = process.platform === "win32";

let failures = 0;
function check(name, cond, detail = "") {
  const ok = !!cond;
  if (!ok) failures++;
  console.log(`  ${ok ? "✓ PASS" : "✗ FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
}

async function waitForServer(url, ms = 60_000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      const r = await fetch(url);
      if (r.ok) return;
    } catch {}
    await sleep(500);
  }
  throw new Error(`mock server never came up at ${url}`);
}
function killTree(child) {
  if (!child || child.killed) return;
  if (isWin) spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
  else try { process.kill(-child.pid, "SIGKILL"); } catch {}
}

// Trigger a connect that rejects, then return the newest toast {title, message}.
async function connectAndReadToast(page, profileId) {
  return page.evaluate(async (id) => {
    window.__demo.useToasts.getState().toasts.forEach((t) =>
      window.__demo.useToasts.getState().dismiss(t.id)
    );
    try {
      await window.__demo.useConnections.getState().connect(id);
    } catch {
      // expected — the store re-throws
    }
    const toasts = window.__demo.useToasts.getState().toasts;
    const t = toasts[toasts.length - 1];
    return t ? { title: t.title, message: t.message ?? "", variant: t.variant } : null;
  }, profileId);
}

async function main() {
  console.log("› starting mock Vite server…");
  const server = spawn("npm", ["run", "dev:mock"], {
    cwd: ROOT,
    shell: true,
    stdio: "ignore",
    detached: !isWin,
  });
  let browser;
  try {
    await waitForServer(URL);
    browser = await puppeteer.launch({
      executablePath: chrome,
      headless: true,
      defaultViewport: { width: 1440, height: 900, deviceScaleFactor: 1 },
      args: ["--hide-scrollbars"],
    });
    const page = await browser.newPage();
    page.on("pageerror", (e) => {
      console.log("  page error:", e.message);
      failures++;
    });
    await page.goto(URL, { waitUntil: "networkidle0" });
    await page.waitForFunction(() => !!window.__demo?.useToasts, { timeout: 15_000 });
    await sleep(500);

    // ---- 1. Structured auth error → toast keyed off kind:"auth" ----
    const authT = await connectAndReadToast(page, "__auth_fail__");
    check("auth error surfaces an error toast", authT?.variant === "error", JSON.stringify(authT));
    check(
      "auth toast shows the kind-specific hint (not just the message)",
      !!authT && authT.message.includes("Reconnect or re-enter your credentials"),
      authT?.message
    );
    check(
      "auth toast still carries the human message",
      !!authT && authT.message.includes("Authentication failed for user demo"),
      authT?.message
    );
    await page.screenshot({ path: path.join(OUT, "verify-err-auth.png") });

    // ---- 2. Structured network error → the network hint, not the auth one ----
    const netT = await connectAndReadToast(page, "__net_fail__");
    check(
      "network toast shows the network hint",
      !!netT && netT.message.includes("Check the connection and try again"),
      netT?.message
    );
    check(
      "network toast is NOT keyed as auth",
      !!netT && !netT.message.includes("Reconnect or re-enter"),
      netT?.message
    );

    // ---- 3. Legacy string error → generic fallback (message verbatim) ----
    const strT = await connectAndReadToast(page, "__str_fail__");
    check(
      "legacy string error shows the raw message",
      !!strT && strT.message.includes("legacy string error: something broke"),
      strT?.message
    );
    check(
      "legacy string error gets no kind-specific hint",
      !!strT &&
        !strT.message.includes("Reconnect or re-enter") &&
        !strT.message.includes("Check the connection"),
      strT?.message
    );

    if (failures === 0) console.log("\n✅ all structured-error checks passed");
    else console.log(`\n❌ ${failures} check(s) failed`);
  } finally {
    if (browser) await browser.close();
    killTree(server);
  }
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error("verify crashed:", e);
  process.exit(3);
});
