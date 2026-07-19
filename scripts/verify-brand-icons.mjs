// Headless runtime verification for Plan 14 — Iconify brand/protocol logos.
// Spins up the mock Vite build (no Rust), drives the real UI, and asserts:
//   • the rail connection bubbles render a brand logo AND keep their monogram,
//   • colour marks (logos:aws-s3) render in colour while mono glyphs (mdi:ssh)
//     follow currentColor,
//   • the connection-list search popover rows show brand logos,
//   • the New-Connection protocol picker shows a brand logo per protocol,
//   • the page makes ZERO requests to iconify.design (fully offline).
// Exit code 0 = all passed.
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
  "C:/Users/Juan/AppData/Local/Temp/claude/C--Users-Juan-Documents-GitHub-Faro/fc5249f7-923a-4736-ace4-f4d33177cbca/scratchpad";
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
  const iconifyRequests = [];
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
    // The whole point of the offline design: nothing may hit the Iconify API.
    page.on("request", (r) => {
      if (/iconify\.design/i.test(r.url())) iconifyRequests.push(r.url());
    });
    await page.goto(URL, { waitUntil: "networkidle0" });

    const drive = (fn, ...a) => page.evaluate(fn, ...a);
    const shot = async (name) => {
      await page.screenshot({ path: path.join(OUT, `brand-${name}.png`) });
      console.log("    · shot", `brand-${name}.png`);
    };

    // Wait for the rail bubbles to mount.
    await page.waitForFunction(
      () => document.querySelectorAll('button[aria-label="api-prod"]').length > 0,
      { timeout: 15_000 }
    );
    await sleep(500);
    await shot("1-rail");

    // ---- 1. Rail: a brand logo on the connection bubbles ----
    const railBrandIcons = await drive(
      () => document.querySelectorAll("svg.brand-icon").length
    );
    // 6 mock connections (3 sftp, 1 ftp, 2 s3) → ≥6 brand chips on the rail.
    check("rail bubbles render brand logos", railBrandIcons >= 6, `svg.brand-icon=${railBrandIcons}`);

    // ---- 2. The colour monogram is NOT lost (logo is additive) ----
    const monogramKept = await drive(() => {
      const b = document.querySelector('button[aria-label="acme-backups"]');
      return !!b && /\bAB\b/.test(b.innerText);
    });
    check("monogram identity preserved beside the logo", monogramKept);

    // ---- 3. Colour brand mark (S3) actually renders in colour ----
    const s3IsColour = await drive(() => {
      const b = document.querySelector('button[aria-label="acme-backups"]');
      const svg = b && b.querySelector("svg.brand-icon");
      return !!svg && /#[0-9a-fA-F]{3,6}/.test(svg.outerHTML);
    });
    check("S3 bubble shows the colour AWS logo (hex fills present)", s3IsColour);

    // ---- 4. Mono protocol glyph (SSH) follows currentColor ----
    const sshIsMono = await drive(() => {
      const b = document.querySelector('button[aria-label="api-prod"]');
      const svg = b && b.querySelector("svg.brand-icon");
      return !!svg && /currentColor/.test(svg.outerHTML);
    });
    check("SFTP bubble shows a currentColor SSH glyph", sshIsMono);

    // ---- 5. Connection-list search popover rows show brand logos ----
    await drive(() => {
      const btn = document.querySelector('button[aria-label^="Search servers"]');
      btn && btn.click();
    });
    await page.waitForFunction(
      () => !!document.querySelector('input[aria-label="Search servers"]'),
      { timeout: 8_000 }
    );
    await sleep(400);
    const searchBrandIcons = await drive(() => {
      const pop = document
        .querySelector('input[aria-label="Search servers"]')
        .closest(".anim-modal");
      return pop ? pop.querySelectorAll("svg.brand-icon").length : -1;
    });
    check("search popover rows show brand logos", searchBrandIcons >= 6, `svg.brand-icon=${searchBrandIcons}`);
    await shot("2-search");
    // Close the popover.
    await page.keyboard.press("Escape");
    await sleep(300);

    // ---- 6. New-Connection protocol picker shows a logo per protocol ----
    await drive(() => {
      const btn = document.querySelector('button[aria-label="New connection"]');
      btn && btn.click();
    });
    await page.waitForFunction(() => !!document.querySelector('[role="dialog"]'), {
      timeout: 8_000,
    });
    await sleep(500);
    const picker = await drive(() => {
      const dlg = document.querySelector('[role="dialog"]');
      const icons = dlg.querySelectorAll("nav svg.brand-icon").length;
      // The S3 protocol button should carry the colour AWS mark.
      const s3btn = [...dlg.querySelectorAll("nav button")].find(
        (b) => b.textContent.trim() === "S3"
      );
      const s3colour =
        s3btn &&
        /#[0-9a-fA-F]{3,6}/.test(s3btn.querySelector("svg.brand-icon")?.outerHTML || "");
      return { icons, s3colour: !!s3colour };
    });
    // 13 protocols in the grouped picker.
    check("protocol picker shows a brand logo per protocol", picker.icons >= 13, `svg.brand-icon=${picker.icons}`);
    check("picker's S3 entry uses the colour AWS logo", picker.s3colour);
    await shot("3-picker");

    // ---- 7. Zero network calls to the Iconify API (fully offline) ----
    check(
      "no requests to iconify.design (offline)",
      iconifyRequests.length === 0,
      iconifyRequests.length ? iconifyRequests.join(", ") : "none"
    );

    if (failures === 0) console.log("\n✅ all brand-icon checks passed");
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
