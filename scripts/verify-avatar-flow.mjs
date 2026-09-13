// Avatar / profile-image verification for desktop-todo-widget v1.0.1.
//
// Runs the real picker (the native common dialog the product opens), the real
// Settings panel, the real save path, the real restart, and inspects what ends up
// on disk, so the matrix below is evidence about the shipped product rather than
// about a mock.
//
// IMPORTANT: the product resolves its data directory through the Windows known
// folder (%APPDATA%\net.alanfloyd.desktop), so a scratch profile cannot be
// injected through the environment. Run this through scripts/verify-avatar-flow.ps1,
// which stashes the real profile behind a verified copy and restores it; the
// `--fresh-profile-ok` flag it passes is what authorises treating that path as
// scratch.
//
// Usage:
//   pwsh -File scripts/verify-avatar-flow.ps1
//   pwsh -File scripts/verify-avatar-flow.ps1 -KeepFixtures
//
// Fixtures are synthetic (a hand-written PNG encoder plus canvas-generated
// JPEG/WebP). No user image is read.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const args = process.argv.slice(2);
function argValue(name, fallback = null) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}
const exe = argValue("--exe", join(repoRoot, "src-tauri", "target", "release", "alan-desktop.exe"));
const dataDir = argValue("--data-dir", join(process.env.APPDATA ?? "", "net.alanfloyd.desktop"));
const fixturesDir = argValue("--fixtures", join(tmpdir(), "dtw-avatar-fixtures"));
const keepFixtures = args.includes("--keep-fixtures");
const PORT = Number(process.env.CDP_PORT ?? 9333);
// The release payload ships the binary under its public product name while the
// cargo build output keeps the internal one, so both are recognised.
const PROCESS_NAMES = ["alan-desktop.exe", "desktop-todo-widget.exe"];
const pickerDriver = join(repoRoot, "scripts", "avatar-picker-drive.ps1");
const WINDOWS_POWERSHELL = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe");

if (!args.includes("--fresh-profile-ok")) {
  console.error(
    "refusing to run: the product's profile is the real %APPDATA%\\net.alanfloyd.desktop, which cannot be\n" +
      "redirected through the environment. Run scripts/verify-avatar-flow.ps1, which stashes and restores the\n" +
      "real profile around this script, or pass --fresh-profile-ok only when that is already done.",
  );
  process.exit(2);
}
if (!existsSync(exe)) {
  console.error(`release executable not found: ${exe}\nBuild it with: pnpm tauri build --no-bundle`);
  process.exit(2);
}

const checks = [];
const notes = [];
function check(name, pass, detail = "") {
  checks.push({ name, pass: Boolean(pass) });
  console.log(`  ${pass ? "PASS" : "FAIL"}  ${name}${detail ? `  ${pass ? "" : detail}` : ""}`);
}
function assertEqual(name, actual, expected) {
  check(name, actual === expected, `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}
function note(message) {
  notes.push(message);
  console.log(`  note  ${message}`);
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// --- fixture generation ------------------------------------------------------
const CRC_TABLE = (() => {
  const table = new Int32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c;
  }
  return table;
})();
function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function pngChunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const typeAndData = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typeAndData));
  return Buffer.concat([length, typeAndData, crc]);
}
/** Minimal RGBA PNG encoder, so fixtures need no image dependency. */
function encodePng(width, height, rgba) {
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y += 1) {
    raw[y * (stride + 1)] = 0; // filter: none
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // colour type: RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", deflateSync(raw, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
}
function makePixels(width, height, painter) {
  const rgba = Buffer.alloc(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const [r, g, b, a] = painter(x, y);
      const offset = (y * width + x) * 4;
      rgba[offset] = r;
      rgba[offset + 1] = g;
      rgba[offset + 2] = b;
      rgba[offset + 3] = a;
    }
  }
  return rgba;
}
function flatPainter(base, accent) {
  return (x, y) => {
    const inside = x > 0 && y > 0 && x < 64 && y < 64 && (x + y) % 17 < 9;
    return inside ? accent : base;
  };
}
/** Deterministic noise: incompressible, which is what makes a big file. */
function noisePainter(seed) {
  let state = seed >>> 0;
  const next = () => {
    state = (state * 1664525 + 1013904223) >>> 0;
    return (state >>> 24) & 0xff;
  };
  return () => [next(), next(), next(), 255];
}
function writeFixture(name, bytes) {
  const path = join(fixturesDir, name);
  writeFileSync(path, bytes);
  return path;
}

// --- app control -------------------------------------------------------------
function launchApp() {
  const child = spawn(exe, [], {
    env: { ...process.env, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` },
    detached: true,
    stdio: "ignore",
  });
  child.unref();
  return child.pid;
}
function productPids() {
  const pids = [];
  for (const name of PROCESS_NAMES) {
    try {
      const output = execFileSync("tasklist", ["/FI", `IMAGENAME eq ${name}`, "/NH", "/FO", "CSV"], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      });
      for (const line of output.split(/\r?\n/)) {
        if (!line.includes(name)) continue;
        const pid = Number(line.split('","')[1]?.replace(/"/g, ""));
        if (Number.isInteger(pid) && pid > 0) pids.push(pid);
      }
    } catch {
      return null;
    }
  }
  return pids;
}

let socket = null;
let exceptions = [];
async function pageTarget() {
  try {
    const response = await fetch(`http://127.0.0.1:${PORT}/json/list`);
    const targets = await response.json();
    return targets.find((t) => t.type === "page" && (t.url ?? "").includes("tauri.localhost")) ?? null;
  } catch {
    return null;
  }
}
let nextId = 1;
function send(method, params = {}) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const onMessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.method === "Runtime.exceptionThrown") {
        exceptions.push(message.params?.exceptionDetails?.exception?.description ?? "unknown exception");
      }
      if (message.id !== id) return;
      socket.removeEventListener("message", onMessage);
      if (message.error) reject(new Error(`${method}: ${message.error.message}`));
      else resolve(message.result);
    };
    socket.addEventListener("message", onMessage);
    socket.send(JSON.stringify({ id, method, params }));
    setTimeout(() => {
      socket.removeEventListener("message", onMessage);
      reject(new Error(`${method}: timed out`));
    }, 30000);
  });
}
async function evaluate(expression) {
  const result = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) {
    throw new Error(`evaluate failed: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
  }
  return result.result.value;
}
async function waitFor(expression, predicate, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  let last;
  while (Date.now() < deadline) {
    last = await evaluate(expression);
    if (predicate(last)) return last;
    await sleep(200);
  }
  return last;
}
async function connectToApp(timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  let target = null;
  while (Date.now() < deadline && !target) {
    target = await pageTarget();
    if (!target) await sleep(500);
  }
  if (!target) throw new Error("no product DevTools page target appeared");
  socket = await new Promise((resolve, reject) => {
    const ws = new WebSocket(target.webSocketDebuggerUrl);
    ws.addEventListener("open", () => resolve(ws));
    ws.addEventListener("error", () => reject(new Error("devtools websocket failed")));
  });
  await send("Runtime.enable");
  const ready = await waitFor(`typeof window.__TAURI_INTERNALS__ !== 'undefined'`, (value) => value === true, timeoutMs);
  if (ready !== true) throw new Error("the product page never exposed the Tauri bridge");
}
async function quitAndWait(pid, label) {
  await evaluate(`(() => { window.__TAURI_INTERNALS__.invoke('quit_app'); return true; })()`);
  for (let attempt = 0; attempt < 40; attempt += 1) {
    await sleep(500);
    try {
      process.kill(pid, 0);
    } catch {
      check(`${label} process exited`, true);
      return true;
    }
  }
  check(`${label} process exited`, false, `pid ${pid} still alive`);
  return false;
}

/** Runs the UIA picker helper and parses its single-line JSON result. */
function runPickerDriver(modeArgs, timeoutMs = 60000) {
  const output = execFileSync(WINDOWS_POWERSHELL, [
    "-NoProfile",
    "-ExecutionPolicy", "Bypass",
    "-File", pickerDriver,
    ...modeArgs,
  ], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: timeoutMs });
  const line = output.trim().split(/\r?\n/).filter(Boolean).pop() ?? "";
  try {
    return JSON.parse(line);
  } catch {
    return { ok: false, reason: `unparseable driver output: ${line}` };
  }
}

const SETTINGS_STATE = `(() => {
  const panel = document.querySelector('.settings-panel');
  const preview = document.querySelector('.settings-avatar-preview img');
  const error = panel ? panel.querySelector('.asset-error') : null;
  const errorRect = error ? error.getBoundingClientRect() : null;
  const row = panel ? panel.querySelector('.avatar-settings-control') : null;
  const rowRect = row ? row.getBoundingClientRect() : null;
  const remove = panel ? panel.querySelector('.avatar-settings-control button:last-child') : null;
  return {
    panelOpen: Boolean(panel),
    previewAvailable: Boolean(preview),
    // Element presence is not rendering: an image the CSP refuses still creates an
    // <img> with naturalWidth 0, which is exactly how a broken avatar used to look.
    previewPixels: preview ? preview.naturalWidth : 0,
    previewSrcPrefix: preview ? preview.src.slice(0, 24) : null,
    errorText: error ? error.textContent.trim() : null,
    // "Visible" means inside the viewport, not merely present in the DOM: a
    // message below the fold is not feedback.
    errorVisible: Boolean(errorRect) && errorRect.height > 0 && errorRect.top >= 0 && errorRect.bottom <= window.innerHeight,
    // Distance from the control that produced it: the row must own the message.
    errorDistanceFromRow: errorRect && rowRect ? Math.round(Math.abs(errorRect.top - rowRect.bottom)) : null,
    removeDisabled: remove ? remove.disabled : null,
  };
})()`;

/** Reads a stored avatar back through the product and measures what it decoded to. */
const storedAssetState = (assetId) => `window.__TAURI_INTERNALS__.invoke('load_managed_asset', {
  kind: 'avatar', assetId: ${JSON.stringify(assetId)}
}).then(async (payload) => {
  if (!payload || !payload.available) return { available: false };
  const image = new Image();
  const loaded = new Promise((resolve) => { image.onload = () => resolve(true); image.onerror = () => resolve(false); });
  image.decoding = 'sync';
  image.src = payload.dataUrl;
  if (!(await loaded)) return { available: false, reason: 'decode failed' };
  const canvas = document.createElement('canvas');
  canvas.width = image.naturalWidth;
  canvas.height = image.naturalHeight;
  const context = canvas.getContext('2d', { willReadFrequently: true });
  context.drawImage(image, 0, 0);
  const corner = context.getImageData(0, 0, 1, 1).data;
  return {
    available: true,
    width: image.naturalWidth,
    height: image.naturalHeight,
    cornerAlpha: corner[3],
    dataUrlLength: payload.dataUrl.length,
    mime: (payload.dataUrl.split(';')[0] || '').replace('data:', ''),
  };
})`;

const PRODUCT_STATE = `window.__TAURI_INTERNALS__.invoke('product_state').then(s => ({
  mode: s.settings.mode,
  presentation: s.settings.floatingPresentation,
  avatarAssetId: s.settings.avatarAssetId,
  displayName: s.settings.displayName,
  material: s.settings.appearanceProfiles[s.settings.mode].backgroundType,
  databasePath: s.databasePath,
}))`;

const RENDER_STATE = `(() => {
  const orb = document.querySelector('.floating-orb img');
  const identity = document.querySelector('.profile-avatar img');
  return {
    // Loaded, not merely present: see the note in SETTINGS_STATE.
    orbAvatar: Boolean(orb) && orb.naturalWidth > 0,
    orbPixels: orb ? orb.naturalWidth : 0,
    identityAvatar: Boolean(identity) && identity.naturalWidth > 0,
    identityPixels: identity ? identity.naturalWidth : 0,
    initialsFallback: Boolean(document.querySelector('.profile-avatar span')),
    appError: document.querySelector('.app-error')?.textContent?.trim() ?? null,
    panelOpen: Boolean(document.querySelector('.settings-panel')),
  };
})()`;

/** Loads a data: URL image in the page and reports what the renderer made of it. */
const IMAGE_LOAD_PROBE = `(async () => {
  const tiny = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8AAAwAB/AF+7gAAAABJRU5ErkJggg==';
  const violations = [];
  const listener = (event) => violations.push({ directive: event.violatedDirective, blocked: (event.blockedURI || '').slice(0, 20) });
  document.addEventListener('securitypolicyviolation', listener);
  const result = await new Promise((resolve) => {
    const image = new Image();
    image.onload = () => resolve({ loaded: true, width: image.naturalWidth });
    image.onerror = () => resolve({ loaded: false, width: 0 });
    image.src = tiny;
  });
  document.removeEventListener('securitypolicyviolation', listener);
  return { ...result, imgSrcViolations: violations.filter((entry) => entry.directive.startsWith('img-src')) };
})()`;

/** Reads the rendered background material and whether its image actually decoded. */
const BACKGROUND_STATE = `(async () => {
  const material = document.querySelector('.appearance-material');
  const layer = document.querySelector('.appearance-background-layer');
  const panel = document.querySelector('.settings-panel');
  const error = panel ? panel.querySelector('.asset-error') : null;
  const computed = layer ? getComputedStyle(layer).backgroundImage : '';
  const url = computed === 'none' ? '' : computed.replace(/^url\\(["']?/, '').replace(/["']?\\)$/, '');
  let loaded = false;
  if (url.startsWith('data:')) {
    loaded = await new Promise((resolve) => {
      const image = new Image();
      image.onload = () => resolve(image.naturalWidth > 0);
      image.onerror = () => resolve(false);
      image.src = url;
    });
  }
  return {
    backgroundType: material ? material.dataset.backgroundType : null,
    hasImageUrl: url.startsWith('data:'),
    loaded,
    error: error ? error.textContent.trim() : null,
    panelOpen: Boolean(panel),
  };
})()`;

async function clickAt(x, y) {
  for (const type of ["mousePressed", "mouseReleased"]) {
    await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  }
}

async function clickSelector(selector) {
  const point = await evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) return null;
    element.scrollIntoView({ block: 'center' });
    const r = element.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
  })()`);
  if (!point) throw new Error(`no element for ${selector}`);
  await clickAt(point.x, point.y);
}

/** Clicks the avatar/background "Choose image" control, retrying until the dialog opens. */
async function openPickerAndSelect(kind, filePath) {
  const buttonSelector = kind === "avatar"
    ? ".avatar-settings-control button:nth-of-type(1)"
    : ".asset-actions button:first-of-type";
  let opened = false;
  for (let attempt = 0; attempt < 3 && !opened; attempt += 1) {
    await clickSelector(buttonSelector);
    const waited = runPickerDriver(["-Mode", "Wait", "-TimeoutSeconds", "8"]);
    opened = waited.found === true;
  }
  if (!opened) return { ok: false, reason: "dialog never appeared" };
  return runPickerDriver(["-Mode", "Select", "-DialogPath", filePath]);
}

function listAssets() {
  const dir = join(dataDir, "assets");
  if (!existsSync(dir)) return [];
  return readdirSync(dir).filter((name) => name.startsWith("avatar-") || name.startsWith("background-"));
}

async function waitForSettingsPanel(timeoutMs = 15000) {
  const ready = await waitFor(SETTINGS_STATE, (state) => state.panelOpen, timeoutMs);
  return ready.panelOpen;
}

/**
 * Opens the Settings panel, retrying the action.
 *
 * The native `open-settings` event is emitted by the backend, and the frontend
 * only subscribes to it once Vue has mounted and its startup loads have finished.
 * A single attempt right after the Tauri bridge appears can therefore land before
 * the listener exists, so the action is repeated until the panel is there.
 */
async function openSettingsPanel(attempts = 5, perAttemptMs = 6000) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    await evaluate(`window.__TAURI_INTERNALS__.invoke('product_action', { action: 'settings' })`);
    if (await waitForSettingsPanel(perAttemptMs)) return true;
  }
  return false;
}

async function saveSettings() {
  await clickSelector(".save-settings");
  await waitFor(SETTINGS_STATE, (state) => !state.panelOpen, 15000);
  await sleep(400);
}

/** Clicks the first settings button whose visible text matches one of `labels`. */
async function clickByLabel(labels) {
  const point = await evaluate(`(() => {
    const wanted = ${JSON.stringify(labels)};
    const button = [...document.querySelectorAll('.settings-panel button')]
      .find((candidate) => wanted.includes((candidate.textContent || '').trim()));
    if (!button) return null;
    button.scrollIntoView({ block: 'center' });
    const rect = button.getBoundingClientRect();
    return { x: Math.round(rect.left + rect.width / 2), y: Math.round(rect.top + rect.height / 2), text: button.textContent.trim() };
  })()`);
  if (!point) throw new Error(`no settings button labelled ${labels.join(" / ")}`);
  await clickAt(point.x, point.y);
  return point;
}

async function launchAndOpenSettings() {
  const pid = launchApp();
  await connectToApp();
  // Wait for the product surface itself, not just the bridge, before asking the
  // backend to open Settings.
  await waitFor(`Boolean(document.querySelector('.product-content, .floating-orb'))`, (value) => value === true, 25000);
  const open = await openSettingsPanel();
  return { pid, open };
}

// --- fixtures ----------------------------------------------------------------
console.log("desktop-todo-widget avatar verification");
console.log(`  exe      : ${exe}`);
console.log(`  profile  : ${dataDir}  (real profile stashed by the wrapper)`);
console.log(`  fixtures : ${fixturesDir}`);
console.log("");

const already = productPids();
if (already && already.length > 0) {
  console.error(`a product instance is already running (pid ${already.join(", ")}); stop it first`);
  process.exit(2);
}

mkdirSync(fixturesDir, { recursive: true });
const fixtures = [];
fixtures.push({ key: "small-png", label: "A 64x64 PNG (<20 KB)", path: writeFixture("a-64.png", encodePng(64, 64, makePixels(64, 64, flatPainter([30, 90, 160, 255], [245, 245, 245, 255])))), expect: "success" });
fixtures.push({ key: "normal-png", label: "B 256x256 PNG", path: writeFixture("b-256.png", encodePng(256, 256, makePixels(256, 256, flatPainter([20, 120, 90, 255], [255, 255, 255, 255])))), expect: "success" });
fixtures.push({ key: "alpha-png", label: "E 256x256 PNG with alpha", path: writeFixture("e-alpha.png", encodePng(256, 256, makePixels(256, 256, (x, y) => {
  const dx = x - 128;
  const dy = y - 128;
  return dx * dx + dy * dy < 110 * 110 ? [235, 120, 60, 255] : [0, 0, 0, 0];
}))), expect: "success", cornerAlpha: 0 });
fixtures.push({ key: "normal-jpeg", label: "C 512x512 JPEG", path: writeFixture("c-512.jpg", Buffer.alloc(0)), expect: "success", canvas: { width: 512, height: 512, type: "image/jpeg", quality: 0.9 } });
fixtures.push({ key: "webp", label: "F 256x256 WebP", path: writeFixture("f-256.webp", Buffer.alloc(0)), expect: "success", canvas: { width: 256, height: 256, type: "image/webp", quality: 0.9 } });
fixtures.push({ key: "large-jpeg", label: "D 2400x1800 JPEG (multi-MB photo)", path: writeFixture("d-2400.jpg", Buffer.alloc(0)), expect: "success", canvas: { width: 2400, height: 1800, type: "image/jpeg", quality: 0.92 } });
// Over the avatar's old 8 MB artifact limit, under the 24 MB source limit: this is
// the "a real photo used to be refused" case, and it must now be normalized and
// stored as a small square instead of rejected.
fixtures.push({ key: "oversized-png", label: "G 2400x1800 PNG over 8 MB (must normalize)", path: writeFixture("g-oversized.png", encodePng(2400, 1800, makePixels(2400, 1800, noisePainter(12345)))), expect: "success" });
// Above the source cap: refused, with a message the user can actually see.
fixtures.push({ key: "above-source-cap", label: "I 3400x2600 PNG over the 24 MB source cap", path: writeFixture("i-huge.png", encodePng(3400, 2600, makePixels(3400, 2600, noisePainter(777)))), expect: "reject", errorPattern: /too large to open|过大/i });
// A real 1x1 GIF89a: a format the UI does not claim, used for the reject path.
const GIF_1X1 = Buffer.from("R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7", "base64");
fixtures.push({ key: "unsupported-gif", label: "H 1x1 GIF (unsupported format)", path: writeFixture("h-unsupported.gif", GIF_1X1), expect: "reject", errorPattern: /unsupported image format|不支持的图片格式/i });

for (const fixture of fixtures) {
  const size = statSync(fixture.path).size;
  note(`${fixture.key}: ${size} bytes (${fixture.label})`);
}

let pid = 0;
try {
  const session = await launchAndOpenSettings();
  pid = session.pid;
  check("Settings panel opens", session.open);

  const freshState = await evaluate(PRODUCT_STATE);
  assertEqual("fresh profile is Floating", freshState.mode, "floating");
  assertEqual("fresh profile is Expanded", freshState.presentation, "expanded");

  // The prerequisite every avatar, custom background, and wallpaper depends on:
  // the product stores user images as data URLs, so the page must be allowed to
  // render them. Without this the picker succeeds and nothing is ever visible.
  const imageProbe = await evaluate(IMAGE_LOAD_PROBE);
  check("the page can render data: images",
    imageProbe.loaded === true && imageProbe.width > 0 && imageProbe.imgSrcViolations.length === 0,
    JSON.stringify(imageProbe));

  // Canvas-generated fixtures (JPEG/WebP) come from the app's own Chromium, so no
  // image library is added to the repository for QA either.
  for (const fixture of fixtures.filter((entry) => entry.canvas)) {
    const dataUrl = await evaluate(`(() => {
      const { width, height, type, quality } = ${JSON.stringify(fixture.canvas)};
      const canvas = document.createElement('canvas');
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext('2d');
      const gradient = context.createLinearGradient(0, 0, width, height);
      gradient.addColorStop(0, '#1b4b6b');
      gradient.addColorStop(0.5, '#c8b48a');
      gradient.addColorStop(1, '#3a2028');
      context.fillStyle = gradient;
      context.fillRect(0, 0, width, height);
      // Photographic detail: noise blocks make the encoded file realistically big.
      for (let i = 0; i < width * height / 64; i += 1) {
        const x = Math.random() * width;
        const y = Math.random() * height;
        context.fillStyle = 'rgba(' + Math.floor(Math.random() * 255) + ',' + Math.floor(Math.random() * 255) + ',' + Math.floor(Math.random() * 255) + ',0.5)';
        context.fillRect(x, y, 3, 3);
      }
      return canvas.toDataURL(type, quality);
    })()`);
    const base64 = dataUrl.split(",")[1] ?? "";
    writeFileSync(fixture.path, Buffer.from(base64, "base64"));
    note(`${fixture.key}: ${statSync(fixture.path).size} bytes after canvas encoding`);
  }

  // --- matrix ---------------------------------------------------------------
  let lastSuccessful = null;
  for (const fixture of fixtures) {
    console.log(`fixture ${fixture.key} (${fixture.label})`);
    const productBefore = await evaluate(PRODUCT_STATE);
    const assetsBefore = listAssets();
    const result = await openPickerAndSelect("avatar", fixture.path);
    check(`${fixture.key}: picker dialog handled`, result.ok === true, JSON.stringify(result));
    note(`${fixture.key}: picker reported path=${result.path ?? "<none>"} on-disk=${statSync(fixture.path).size} bytes`);
    await sleep(1200);

    const after = await evaluate(SETTINGS_STATE);
    const assets = listAssets();
    const state = await evaluate(PRODUCT_STATE);

    if (fixture.expect === "success") {
      check(`${fixture.key}: preview renders the picked image`, after.previewPixels === 256,
        `naturalWidth=${after.previewPixels} state=${JSON.stringify(after)}`);
      check(`${fixture.key}: no error message`, !after.errorText, `error=${after.errorText}`);
      check(`${fixture.key}: managed asset written`, assets.length > 0, `assets=${assets.join(",")}`);
      await saveSettings();
      const savedAssets = listAssets();
      const savedState = await evaluate(PRODUCT_STATE);
      const render = await evaluate(RENDER_STATE);
      check(`${fixture.key}: asset survives Save`, savedAssets.length > 0, `assets=${savedAssets.join(",")}`);
      check(`${fixture.key}: settings reference the new asset`,
        Boolean(savedState.avatarAssetId) && savedAssets.includes(savedState.avatarAssetId),
        `id=${savedState.avatarAssetId} assets=${savedAssets.join(",")}`);
      check(`${fixture.key}: expanded identity shows the avatar`, render.identityAvatar === true, JSON.stringify(render));
      check(`${fixture.key}: no application error`, !render.appError, `appError=${render.appError}`);

      // What was persisted is the normalized artifact, not the original: a square
      // PNG at the documented edge, small on disk no matter how big the source was.
      const stored = await evaluate(storedAssetState(savedState.avatarAssetId));
      const sourceBytes = statSync(fixture.path).size;
      const storedBytes = statSync(join(dataDir, "assets", savedState.avatarAssetId)).size;
      note(`${fixture.key}: source ${sourceBytes} bytes -> stored ${storedBytes} bytes (${stored.width}x${stored.height} ${stored.mime})`);
      check(`${fixture.key}: stored avatar is square at the documented edge`,
        stored.available === true && stored.width === 256 && stored.height === 256, JSON.stringify(stored));
      check(`${fixture.key}: stored avatar is a PNG`, stored.mime === "image/png", `mime=${stored.mime}`);
      check(`${fixture.key}: stored avatar stays small`, storedBytes <= 512 * 1024, `${storedBytes} bytes`);
      check(`${fixture.key}: the original is not persisted`,
        storedBytes < sourceBytes || sourceBytes < 512 * 1024, `source=${sourceBytes} stored=${storedBytes}`);
      if (fixture.cornerAlpha !== undefined) {
        check(`${fixture.key}: transparency survives normalization`, stored.cornerAlpha === fixture.cornerAlpha,
          `cornerAlpha=${stored.cornerAlpha}`);
      }
      lastSuccessful = { fixture, assetId: savedState.avatarAssetId };
    }
    else {
      // A rejected image must produce a visible, localized message and must not
      // leave a half-applied avatar or an orphan file behind.
      const errorShown = Boolean(after.errorText);
      check(`${fixture.key}: rejection reports an error`, errorShown, JSON.stringify(after));
      check(`${fixture.key}: the error is visible without scrolling`, after.errorVisible === true,
        `visible=${after.errorVisible} distanceFromRow=${after.errorDistanceFromRow}`);
      check(`${fixture.key}: the error is shown by the control that caused it`,
        typeof after.errorDistanceFromRow === "number" && after.errorDistanceFromRow <= 120,
        `distanceFromRow=${after.errorDistanceFromRow}`);
      check(`${fixture.key}: the message is localized, not the raw backend string`,
        fixture.errorPattern instanceof RegExp && fixture.errorPattern.test(after.errorText ?? ""),
        `error=${after.errorText}`);
      const newAssets = assets.filter((name) => !assetsBefore.includes(name));
      check(`${fixture.key}: no orphan asset was written`, newAssets.length === 0, `new=${newAssets.join(",")}`);
      assertEqual(`${fixture.key}: persisted avatar unchanged`, state.avatarAssetId, productBefore.avatarAssetId);
    }
    // A fresh panel for the next fixture keeps the assertions independent.
    if (await evaluate(`Boolean(document.querySelector('.settings-panel'))`) === false) {
      await openSettingsPanel();
    }
  }
  note(`last successful fixture: ${lastSuccessful ? lastSuccessful.fixture.key : "<none>"}`);

  // --- restart persistence --------------------------------------------------
  console.log("restart persistence");
  await openSettingsPanel();
  await openPickerAndSelect("avatar", fixtures.find((entry) => entry.key === "normal-png").path);
  await sleep(1000);
  await saveSettings();
  const beforeRestart = await evaluate(PRODUCT_STATE);
  await quitAndWait(pid, "restart");
  socket?.close();
  pid = launchApp();
  await connectToApp();
  const afterRestart = await evaluate(PRODUCT_STATE);
  assertEqual("avatar asset id survives a restart", afterRestart.avatarAssetId, beforeRestart.avatarAssetId);
  // The asset is re-read over IPC and decoded after mount, so wait for the
  // rendered pixels rather than sampling the DOM immediately.
  const restartRender = await waitFor(RENDER_STATE, (state) => state.identityAvatar, 25000);
  check("expanded identity shows the avatar after a restart", restartRender.identityAvatar === true, JSON.stringify(restartRender));

  // --- Orb rendering --------------------------------------------------------
  console.log("orb");
  await waitFor(RENDER_STATE, (state) => state.identityAvatar, 25000);
  await evaluate(`window.__TAURI_INTERNALS__.invoke('product_action', { action: 'floating.collapse' })`);
  await waitFor(`Boolean(document.querySelector('.floating-orb'))`, (value) => value === true, 25000);
  const orbState = await waitFor(RENDER_STATE, (state) => state.orbAvatar, 25000);
  check("Orb renders the saved avatar", orbState.orbAvatar === true, JSON.stringify(orbState));
  await evaluate(`window.__TAURI_INTERNALS__.invoke('product_action', { action: 'floating.expand' })`);
  await waitFor(RENDER_STATE, (state) => state.identityAvatar, 25000);

  // --- clear -----------------------------------------------------------------
  console.log("clear avatar");
  await openSettingsPanel();
  await clickSelector(".avatar-settings-control button:last-of-type");
  await saveSettings();
  const clearedState = await evaluate(PRODUCT_STATE);
  const clearedRender = await evaluate(RENDER_STATE);
  assertEqual("clearing the avatar removes the setting", clearedState.avatarAssetId, null);
  check("clearing the avatar restores the initials fallback", clearedRender.initialsFallback === true && clearedRender.identityAvatar === false,
    JSON.stringify(clearedRender));
  check("the cleared asset file is removed from disk", listAssets().filter((name) => name === beforeRestart.avatarAssetId).length === 0,
    `assets=${listAssets().join(",")}`);

  // --- background picker ----------------------------------------------------
  // The same native picker and the same command serve the background material,
  // and only the avatar path asks for the non-persisting mode. This phase proves
  // the shared command still behaves as the background flow expects: the pick is
  // stored at full resolution, the material switches to the image, and the image
  // actually renders.
  console.log("background image picker");
  await openSettingsPanel();
  const imageButton = await clickByLabel(["Image", "图片"]);
  console.log(`  selected background type: ${imageButton.text}`);
  await sleep(300);
  const backgroundFixture = fixtures.find((entry) => entry.key === "large-jpeg");
  const backgroundPick = await openPickerAndSelect("background", backgroundFixture.path);
  check("background: picker dialog handled", backgroundPick.ok === true, JSON.stringify(backgroundPick));
  await sleep(1500);
  const backgroundDraft = await evaluate(BACKGROUND_STATE);
  check("background: no picker error", !backgroundDraft.error, `error=${backgroundDraft.error}`);
  // The panel edits a draft, so the material itself changes when the settings are
  // saved — which is also what makes this a check of the stored image rendering.
  await saveSettings();
  const backgroundState = await evaluate(BACKGROUND_STATE);
  check("background: material bound to the picked image",
    backgroundState.backgroundType === "image" && backgroundState.hasImageUrl, JSON.stringify(backgroundState));
  check("background: the stored image actually renders", backgroundState.loaded === true, JSON.stringify(backgroundState));
  const backgroundAssets = listAssets().filter((name) => name.startsWith("background-"));
  const storedBackground = backgroundAssets
    .map((name) => ({ name, bytes: statSync(join(dataDir, "assets", name)).size }))
    .sort((a, b) => b.bytes - a.bytes)[0];
  const sourceBytes = statSync(backgroundFixture.path).size;
  note(`background: source ${sourceBytes} bytes -> stored ${storedBackground?.bytes} bytes (${storedBackground?.name})`);
  check("background: stored at full resolution, not normalized",
    Boolean(storedBackground) && storedBackground.bytes === sourceBytes,
    `stored=${storedBackground?.bytes} source=${sourceBytes}`);
  const backgroundAfterSave = await evaluate(BACKGROUND_STATE);
  check("background: still renders after Save", backgroundAfterSave.loaded === true, JSON.stringify(backgroundAfterSave));

  // --- gear / collapse regression ------------------------------------------
  console.log("regression");
  const gear = await evaluate(`(() => {
    const gears = [...document.querySelectorAll('.footer-settings-button')];
    return { count: gears.length, width: gears[0] ? Math.round(gears[0].getBoundingClientRect().width) : null };
  })()`);
  check("Settings gear is still present in the expanded footer", gear.count === 1 && gear.width === 30, JSON.stringify(gear));

  check("no uncaught JavaScript exception during the run", exceptions.length === 0, exceptions.join(" | "));

  await quitAndWait(pid, "final");
} finally {
  socket?.close();
  for (const stray of productPids() ?? []) {
    try { process.kill(stray); } catch { /* ignore */ }
  }
  await sleep(500);
  if (!keepFixtures) rmSync(fixturesDir, { recursive: true, force: true });
}

const failed = checks.filter((entry) => !entry.pass);
console.log("");
console.log(`${checks.length - failed.length}/${checks.length} checks passed`);
if (failed.length > 0) {
  for (const entry of failed) console.log(`  failed: ${entry.name}`);
  process.exit(1);
}
process.exit(0);
