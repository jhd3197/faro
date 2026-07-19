// Headless runtime verification for Plan 16 Phase 4 (one-click "Add faro-cli to
// PATH"). Spins up the mock Vite build (no Rust), opens Settings → About, and
// drives the PATH row's Add ⇄ Remove flow, asserting on the rendered DOM.
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
  "C:/Users/Juan/AppData/Local/Temp/claude/C--Users-Juan-Documents-GitHub-Faro/f1093608-9381-404b-a304-c789e9021bc6/scratchpad";
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
    const drive = (fn, ...a) => page.evaluate(fn, ...a);
    const shot = async (name) => {
      await page.screenshot({ path: path.join(OUT, `verify-path-${name}.png`) });
      console.log("    · shot", `verify-path-${name}.png`);
    };

    // The first cold load can race Vite's dep pre-optimization (504s → the app
    // never boots on that pass). Reload until the demo bootstrap is present.
    let booted = false;
    for (let attempt = 0; attempt < 5 && !booted; attempt++) {
      await page.goto(URL, { waitUntil: "networkidle0" });
      try {
        await page.waitForFunction(() => !!window.__demo?.useLayout, { timeout: 8_000 });
        booted = true;
      } catch {
        console.log(`    · boot attempt ${attempt + 1} missed __demo, reloading…`);
        await sleep(1500);
      }
    }
    if (!booted) throw new Error("app never exposed window.__demo");
    await sleep(400);

    // ---- 1. Open Settings → About ----
    await drive(() => window.__demo.useLayout.getState().openDialog("settings"));
    await sleep(400);
    const onTab = await drive(() => {
      const btn = [...document.querySelectorAll("nav button")].find(
        (b) => b.innerText.trim() === "About"
      );
      if (btn) btn.click();
      return !!btn;
    });
    check("opened the Settings → About tab", onTab);
    await sleep(400);

    const initial = await drive(() => document.body.innerText);
    check("About tab shows the PATH row", /faro-cli on PATH/i.test(initial));
    check(
      "PATH row starts in the 'not on PATH' state with an Add button",
      /isn't on your PATH/i.test(initial) && /Add to PATH/i.test(initial)
    );
    await shot("1-before");

    // ---- 2. Click "Add to PATH" → row flips to managed + Remove ----
    const clickedAdd = await drive(() => {
      const btn = [...document.querySelectorAll("button")].find(
        (b) => b.innerText.trim() === "Add to PATH"
      );
      if (btn) btn.click();
      return !!btn;
    });
    check("clicked Add to PATH", clickedAdd);
    await sleep(400);
    const afterAdd = await drive(() => document.body.innerText);
    check(
      "after add: row shows managed-on-PATH + a Remove button",
      /managed by Faro/i.test(afterAdd) && /Remove from PATH/i.test(afterAdd)
    );
    check("after add: no stale 'Add to PATH' button remains", !/Add to PATH/.test(afterAdd));
    await shot("2-added");

    // ---- 3. Click "Remove from PATH" → back to the Add state ----
    const clickedRemove = await drive(() => {
      const btn = [...document.querySelectorAll("button")].find(
        (b) => b.innerText.trim() === "Remove from PATH"
      );
      if (btn) btn.click();
      return !!btn;
    });
    check("clicked Remove from PATH", clickedRemove);
    await sleep(400);
    const afterRemove = await drive(() => document.body.innerText);
    check(
      "after remove: row is back to 'not on PATH' with an Add button",
      /isn't on your PATH/i.test(afterRemove) && /Add to PATH/i.test(afterRemove)
    );
    await shot("3-removed");

    console.log(`\n${failures === 0 ? "✓ ALL CHECKS PASSED" : `✗ ${failures} CHECK(S) FAILED`}`);
  } finally {
    if (browser) await browser.close();
    killTree(server);
  }
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
