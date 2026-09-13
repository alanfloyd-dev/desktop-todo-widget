// First-use UX QA for desktop-todo-widget v1.0.1.
//
// Launches the release build as a genuine first run and checks the three things
// this pass changed:
//
//   * a fresh profile opens the widget visibly (Floating expanded, not the Orb),
//   * the visible Settings gear exists in Floating expanded, Sidebar, and
//     Desktop, is absent from the Orb, and opens the existing Settings surface
//     by mouse and by keyboard,
//   * the pre-existing collapse/expand behaviour is unchanged.
//
// IMPORTANT: the product resolves its data directory through the Windows known
// folder (`%APPDATA%\net.alanfloyd.desktop`), not through a caller-supplied
// environment variable, so a scratch profile cannot be injected. Run this script
// through scripts/verify-first-use-ux.ps1, which moves the real profile aside
// behind a verified copy and restores it afterwards; the `--fresh-profile-ok`
// flag it passes is what authorises this script to treat that path as scratch.
//
// Usage (from the repository root):
//   pwsh -File scripts/verify-first-use-ux.ps1
//   pwsh -File scripts/verify-first-use-ux.ps1 -ShotShotPath ...   # see the .ps1
//
// The app is quit through its own `quit_app` command at the end, so the run also
// covers "Quit leaves nothing behind".
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const args = process.argv.slice(2);
function argValue(name, fallback = null) {
  const index = args.indexOf(name);
  return index >= 0 ? args[index + 1] : fallback;
}
const exe = argValue("--exe", join(repoRoot, "src-tauri", "target", "release", "alan-desktop.exe"));
/**
 * Version the release under test must report.
 *
 * Read from `tauri.conf.json` rather than hardcoded, so the check fails loudly if
 * a future bump forgets one of the places the version lives.
 */
const expectedVersion = (() => {
  try {
    const config = JSON.parse(readFileSync(join(repoRoot, "src-tauri", "tauri.conf.json"), "utf8"));
    return String(config.version);
  } catch {
    return "unknown";
  }
})();
const shotPath = argValue("--shot");
const shotSettingsPath = argValue("--shot-settings");
const freshProfileConfirmed = args.includes("--fresh-profile-ok");
const dataDir = argValue("--data-dir", join(process.env.APPDATA ?? "", "net.alanfloyd.desktop"));
const PORT = Number(process.env.CDP_PORT ?? 9333);
// The release payload ships the binary under its public product name while the
// cargo build output keeps the internal one, so both are recognised.
const PROCESS_NAMES = ["alan-desktop.exe", "desktop-todo-widget.exe"];

if (!freshProfileConfirmed) {
  console.error(
    "refusing to run: the product's profile is the real %APPDATA%\\net.alanfloyd.desktop, which cannot be\n" +
      "redirected through the environment. Run scripts/verify-first-use-ux.ps1, which stashes and restores\n" +
      "the real profile around this script, or pass --fresh-profile-ok only when that is already done.",
  );
  process.exit(2);
}
if (!dataDir || dataDir === join("", "net.alanfloyd.desktop")) {
  console.error("cannot resolve the product data directory; pass --data-dir explicitly");
  process.exit(2);
}

const checks = [];
function check(name, pass, detail = "") {
  checks.push({ name, pass: Boolean(pass) });
  console.log(`  ${pass ? "PASS" : "FAIL"}  ${name}${pass ? "" : `  ${detail}`}`);
}
function assertEqual(name, actual, expected) {
  check(name, actual === expected, `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// --- launch ------------------------------------------------------------------
if (!existsSync(exe)) {
  console.error(`release executable not found: ${exe}\nBuild it with: pnpm tauri build --no-bundle`);
  process.exit(2);
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
      // tasklist unavailable: reported as "unknown" by the caller.
      return null;
    }
  }
  return pids;
}

const alreadyRunning = productPids();
if (alreadyRunning && alreadyRunning.length > 0) {
  console.error(`desktop-todo-widget is already running (pid ${alreadyRunning.join(", ")}); quit it first.`);
  process.exit(2);
}

console.log("desktop-todo-widget first-use UX QA");
console.log(`  exe      : ${exe}`);
console.log(`  profile  : ${dataDir}  (real profile stashed by the wrapper)`);
console.log("");

function launchApp(extraEnv = {}) {
  const child = spawn(exe, [], {
    env: {
      ...process.env,
      ...extraEnv,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
    },
    detached: true,
    // stdio must not be a pipe to this process: the shipped binary is a GUI
    // application, and the QA harness should not give it a console of its own.
    stdio: "ignore",
  });
  child.unref();
  return child.pid;
}

// --- DevTools endpoint -------------------------------------------------------
// The product page only: WebView2 registers an initial `about:blank` page target
// before Tauri navigates it to `tauri://localhost`, and that blank page has no
// Tauri bridge at all.
async function pageTarget() {
  try {
    const response = await fetch(`http://127.0.0.1:${PORT}/json/list`);
    const targets = await response.json();
    return targets.find((target) => target.type === "page" && (target.url ?? "").includes("tauri.localhost")) ?? null;
  } catch {
    return null;
  }
}

let socket = null;
async function connectToApp(timeoutMs = 60000) {
  const deadline = Date.now() + timeoutMs;
  let target = null;
  while (Date.now() < deadline && !target) {
    target = await pageTarget();
    if (!target) await sleep(500);
  }
  if (!target) throw new Error("no product DevTools page target appeared; the app did not start");
  console.log(`  devtools target: ${target.url}`);
  socket = await new Promise((resolve, reject) => {
    const ws = new WebSocket(target.webSocketDebuggerUrl);
    ws.addEventListener("open", () => resolve(ws));
    ws.addEventListener("error", () => reject(new Error("devtools websocket failed")));
  });
  // The bridge is injected by Tauri's init script; every check below needs it.
  const ready = await waitFor(`typeof window.__TAURI_INTERNALS__ !== 'undefined'`, (value) => value === true, timeoutMs);
  if (ready !== true) throw new Error("the product page never exposed the Tauri bridge");
}

let nextId = 1;
function send(method, params = {}) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const onMessage = (event) => {
      const message = JSON.parse(event.data);
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
    }, 20000);
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
    await sleep(250);
  }
  return last;
}

const GEAR_STATE = `(() => {
  const gears = [...document.querySelectorAll('.footer-settings-button')];
  const gear = gears[0];
  const rect = gear ? gear.getBoundingClientRect() : null;
  const identity = document.querySelector('.profile-identity');
  const identityRect = identity ? identity.getBoundingClientRect() : null;
  return {
    mode: document.querySelector('.app-shell')?.dataset.windowMode ?? null,
    presentation: document.querySelector('.app-shell')?.dataset.floatingPresentation ?? null,
    gearCount: gears.length,
    orbCount: document.querySelectorAll('.floating-orb').length,
    settingsOpen: Boolean(document.querySelector('.settings-panel')),
    label: gear ? gear.getAttribute('aria-label') : null,
    title: gear ? gear.getAttribute('title') : null,
    tag: gear ? gear.tagName : null,
    width: rect ? Math.round(rect.width) : null,
    height: rect ? Math.round(rect.height) : null,
    iconWidth: gear?.querySelector('svg') ? Math.round(gear.querySelector('svg').getBoundingClientRect().width) : null,
    x: rect ? Math.round(rect.left + rect.width / 2) : null,
    y: rect ? Math.round(rect.top + rect.height / 2) : null,
    right: rect ? Math.round(rect.right) : null,
    bottom: rect ? Math.round(rect.bottom) : null,
    identityRight: identityRect ? Math.round(identityRect.right) : null,
    viewportWidth: window.innerWidth,
    viewportHeight: window.innerHeight,
  };
})()`;

async function gearState() {
  return evaluate(GEAR_STATE);
}

async function runAction(action) {
  // The same IPC command the context menu and the gear's click handler reach
  // through `runAction` in App.vue. Called directly because the production
  // bundle keeps no dev-only access to the component instance.
  await evaluate(
    `window.__TAURI_INTERNALS__.invoke('product_action', { action: ${JSON.stringify(action)} })`,
  );
}

async function clickAt(x, y) {
  for (const type of ["mousePressed", "mouseReleased"]) {
    await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
  }
}

async function clickSelector(selector) {  const rect = await evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) return null;
    element.scrollIntoView({ block: 'center' });
    const r = element.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
  })()`);
  if (!rect) throw new Error(`no element for ${selector}`);
  await clickAt(rect.x, rect.y);
}

async function pressKey(key, code, windowsVirtualKeyCode, text = null) {
  // `rawKeyDown` + `char` + `keyUp` is the sequence Chromium treats as a real
  // keystroke, including the default action a focused button performs on
  // Enter (keydown) and Space (keyup).
  const base = {
    key,
    code,
    windowsVirtualKeyCode,
    nativeVirtualKeyCode: windowsVirtualKeyCode,
  };
  await send("Input.dispatchKeyEvent", { ...base, type: "rawKeyDown" });
  if (text !== null) {
    await send("Input.dispatchKeyEvent", { ...base, type: "char", text, unmodifiedText: text });
  }
  await send("Input.dispatchKeyEvent", { ...base, type: "keyUp" });
}

async function closeSettings() {
  await pressKey("Escape", "Escape", 27);
  await sleep(400);
}

/**
 * Clicks the first settings button whose visible text matches one of `labels`.
 *
 * The appearance controls are localized, so an option is selected by its label in
 * either language rather than by a selector that would encode one translation.
 */
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

/** Quits through the product's own command and waits for the process to vanish. */
async function quitAndWait(targetPid, label) {
  const accepted = await evaluate(`(() => { window.__TAURI_INTERNALS__.invoke('quit_app'); return true; })()`);
  check(`${label} quit command accepted`, accepted === true);
  let exited = false;
  for (let attempt = 0; attempt < 40 && !exited; attempt += 1) {
    await sleep(500);
    try {
      process.kill(targetPid, 0);
    } catch {
      exited = true;
    }
  }
  check(`${label} process exited after Quit`, exited, `pid ${targetPid} still alive`);
  const remaining = productPids();
  check(`${label} no residual product process`, remaining === null || remaining.length === 0,
    `still running: ${remaining?.join(", ")}`);
  return exited;
}

/** Optional visual evidence: the surface exactly as WebView2 renders it. */
async function capture(path) {
  if (!path) return;
  const shot = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(path, Buffer.from(shot.data, "base64"));
  console.log(`  screenshot: ${path}`);
}

/** Reads the authoritative product state, including the database it opened. */
async function reportState(label) {
  const state = await evaluate(`window.__TAURI_INTERNALS__.invoke('product_state')`);
  console.log(`  [${label}] mode=${state?.settings?.mode} presentation=${state?.settings?.floatingPresentation} db=${state?.databasePath}`);
  return state;
}

let pid = launchApp();
try {
  await connectToApp();
  await send("Page.enable");

  // --- A. fresh profile -----------------------------------------------------
  console.log("fresh profile");
  // Wait for the mounted app *and* its stylesheet, so the layout assertions below
  // measure the real 30 px control rather than an unstyled button.
  const fresh = await waitFor(GEAR_STATE, (state) => state.mode !== null && state.width >= 28);
  assertEqual("fresh profile mode", fresh.mode, "floating");
  assertEqual("fresh profile presentation", fresh.presentation, "expanded");
  check("fresh profile is not the Orb", fresh.orbCount === 0);
  assertEqual("fresh profile shows exactly one Settings gear", fresh.gearCount, 1);
  check("gear is a real button", fresh.tag === "BUTTON", `tag=${fresh.tag}`);
  check(
    "gear has the localized accessible name and tooltip",
    (fresh.label === "Settings" || fresh.label === "设置") && fresh.label === fresh.title,
    `aria-label=${fresh.label} title=${fresh.title}`,
  );
  check("gear hit area is 28-32 px", fresh.width >= 28 && fresh.width <= 32 && fresh.height >= 28 && fresh.height <= 32,
    `${fresh.width}x${fresh.height}`);
  check("gear icon is 14-16 px", fresh.iconWidth >= 14 && fresh.iconWidth <= 16, `${fresh.iconWidth}`);
  check("gear sits in the footer's right half", fresh.right >= fresh.viewportWidth - 60 && fresh.right > fresh.viewportWidth / 2,
    `right=${fresh.right} of ${fresh.viewportWidth}`);
  check("gear sits in the footer's bottom band", fresh.bottom >= fresh.viewportHeight - 120 && fresh.bottom <= fresh.viewportHeight,
    `bottom=${fresh.bottom} of ${fresh.viewportHeight}`);
  check("gear does not overlap the profile identity",
    fresh.identityRight === null || fresh.identityRight <= fresh.x,
    `identityRight=${fresh.identityRight} gearX=${fresh.x}`);
  await capture(shotPath);

  // The fresh profile was created on disk at the product's real data path, which
  // the wrapper has stashed for the duration of this run.
  await sleep(500);
  const dbPath = join(dataDir, "alan-desktop.sqlite3");
  const walPath = `${dbPath}-wal`;
  check("fresh profile database created at the product data path", existsSync(dbPath), `missing in ${dataDir}`);
  // The written row may still be in the write-ahead log, so both files are
  // searched: what matters is that the choice reached disk, not which file holds
  // it before the next checkpoint.
  const onDisk = Buffer.concat([
    existsSync(dbPath) ? readFileSync(dbPath) : Buffer.alloc(0),
    existsSync(walPath) ? readFileSync(walPath) : Buffer.alloc(0),
  ]);
  check('fresh profile persisted floatingPresentation="expanded"',
    onDisk.includes('"floatingPresentation":"expanded"'));
  const state = await evaluate(`window.__TAURI_INTERNALS__.invoke('product_state')`);
  assertEqual("product_state reports floating mode", state?.settings?.mode, "floating");
  assertEqual("product_state reports expanded presentation", state?.settings?.floatingPresentation, "expanded");
  assertEqual("product_state reports the Standard rendering backend", state?.settings?.renderingBackend, "standard");

  // Material: the Standard backend has no native Acrylic, so a fresh profile
  // paints the documented Gradient instead of Glass, which keeps its own shape
  // over any wallpaper. The values are the existing Gradient palette, not new
  // colours, and the surface stays translucent.
  const freshProfile = state?.settings?.appearanceProfiles?.[state?.settings?.mode];
  assertEqual("fresh profile uses the Gradient material", freshProfile?.backgroundType, "gradient");
  assertEqual("fresh gradient start colour", freshProfile?.gradientStartColor, "#11191e");
  assertEqual("fresh gradient end colour", freshProfile?.gradientEndColor, "#213747");
  assertEqual("fresh gradient angle", freshProfile?.gradientAngle, 135);
  check("fresh material stays translucent",
    typeof freshProfile?.backgroundOpacity === "number" && freshProfile.backgroundOpacity < 1,
    `backgroundOpacity=${freshProfile?.backgroundOpacity}`);
  check("fresh material is not a flat black rectangle",
    freshProfile?.backgroundOpacity === 0.94 && freshProfile?.overlayStrength === 0.18,
    `opacity=${freshProfile?.backgroundOpacity} overlay=${freshProfile?.overlayStrength}`);

  // The version the user would paste into a bug report has to be the released one.
  const diagnostics = await evaluate(`window.__TAURI_INTERNALS__.invoke('copyable_diagnostics')`);
  const reportedVersion = typeof diagnostics === "string"
    ? (/App version:\s*(\S+)/.exec(diagnostics)?.[1] ?? null)
    : (diagnostics?.appVersion ?? null);
  assertEqual("diagnostics report the released app version", reportedVersion, expectedVersion);

  // Diagnostics still land in the file beside the executable without a console.
  const exeDir = dirname(exe);
  const qaLogs = readdirSync(exeDir).filter((name) => name.startsWith("phase7b-qa-") && name.endsWith(".log"));
  check("QA diagnostics log exists beside the executable", qaLogs.length > 0, `none in ${exeDir}`);
  if (qaLogs.length > 0) {
    const log = readFileSync(join(exeDir, qaLogs[0]), "utf8");
    check("QA diagnostics log records the session", log.includes("qa_session_start=true"));
  }

  // --- B. one gear in every expanded mode -----------------------------------
  for (const [mode, action] of [["sidebar", "mode.sidebar"], ["desktop", "mode.desktop"], ["floating", "mode.floating"]]) {
    console.log(`${mode} expanded`);
    await runAction(action);
    const state = await waitFor(GEAR_STATE, (current) => current.mode === mode);
    assertEqual(`${mode}: window mode applied`, state.mode, mode);
    assertEqual(`${mode}: exactly one Settings gear`, state.gearCount, 1);
    check(`${mode}: gear is visible`, state.width > 0 && state.height > 0);
    check(`${mode}: no Orb rendered`, state.orbCount === 0);
  }

  // --- C. the Orb has no gear ----------------------------------------------
  console.log("orb collapsed");
  await runAction("floating.collapse");
  const orb = await waitFor(GEAR_STATE, (state) => state.presentation === "collapsed");
  assertEqual("orb: presentation collapsed", orb.presentation, "collapsed");
  assertEqual("orb: no Settings gear", orb.gearCount, 0);
  assertEqual("orb: Orb rendered", orb.orbCount, 1);

  // --- D. profile collapse/expand still works (real input) ------------------
  console.log("collapse affordance (regression)");
  await runAction("floating.expand");
  const expanded = await waitFor(GEAR_STATE, (state) => state.presentation === "expanded");
  assertEqual("expand: gear back after expanding", expanded.gearCount, 1);
  await clickSelector(".profile-identity.collapse-affordance");
  const collapsedByClick = await waitFor(GEAR_STATE, (state) => state.presentation === "collapsed");
  assertEqual("clicking the profile identity still collapses to the Orb", collapsedByClick.presentation, "collapsed");
  await clickSelector(".floating-orb");
  const expandedByClick = await waitFor(GEAR_STATE, (state) => state.presentation === "expanded");
  assertEqual("clicking the Orb still expands", expandedByClick.presentation, "expanded");
  assertEqual("expanded again: exactly one gear", expandedByClick.gearCount, 1);

  // --- E. the gear opens the existing Settings surface ---------------------
  console.log("gear opens Settings");
  const beforeClick = await gearState();
  await clickAt(beforeClick.x, beforeClick.y);
  const openedByClick = await waitFor(GEAR_STATE, (state) => state.settingsOpen);
  check("clicking the gear opens the Settings panel", openedByClick.settingsOpen);
  await capture(shotSettingsPath);
  await closeSettings();
  const closed = await gearState();
  check("Escape closes Settings (unchanged close path)", !closed.settingsOpen);

  // --- F. keyboard activation ---------------------------------------------
  console.log("keyboard activation");
  const focusable = await evaluate(`(() => {
    const gear = document.querySelector('.footer-settings-button');
    gear.focus();
    return document.activeElement === gear;
  })()`);
  check("gear is keyboard focusable", focusable);
  await pressKey("Enter", "Enter", 13, "\r");
  const openedByEnter = await waitFor(GEAR_STATE, (state) => state.settingsOpen, 5000);
  check("Enter activates the gear", openedByEnter.settingsOpen);
  await closeSettings();
  const focusableAgain = await evaluate(`(() => {
    const gear = document.querySelector('.footer-settings-button');
    gear.focus();
    return document.activeElement === gear;
  })()`);
  check("gear can be refocused after closing Settings", focusableAgain);
  await pressKey(" ", "Space", 32, " ");
  const openedBySpace = await waitFor(GEAR_STATE, (state) => state.settingsOpen, 5000);
  check("Space activates the gear", openedBySpace.settingsOpen);
  await closeSettings();

  // --- G. a chosen material survives a restart -----------------------------
  // The fresh profile starts on Gradient. Choosing Glass must stick: by this point
  // the profile has a stored document, so the fresh-profile bootstrap must not be
  // able to re-apply its own default over the user's choice.
  console.log("material persistence");
  await evaluate(`window.__TAURI_INTERNALS__.invoke('product_action', { action: 'settings' })`);
  await waitFor(GEAR_STATE, (state) => state.settingsOpen);
  const glassButton = await clickByLabel(["Glass", "玻璃"]);
  console.log(`  selected material: ${glassButton.text}`);
  await clickSelector(".save-settings");
  await waitFor(GEAR_STATE, (state) => !state.settingsOpen, 15000);
  await sleep(400);
  const afterGlass = await evaluate(`window.__TAURI_INTERNALS__.invoke('product_state')`);
  assertEqual("choosing Glass on an existing profile stores Glass",
    afterGlass?.settings?.appearanceProfiles?.[afterGlass?.settings?.mode]?.backgroundType, "glass");

  // --- H. persisted selection survives a restart ---------------------------
  // Leave a deliberate, non-default state behind: collapsed to the Orb. A restart
  // must restore exactly that, which is what proves the fresh-profile default is
  // applied once and never re-derived over the user's own choice.
  console.log("restart with the Orb persisted");
  await runAction("floating.collapse");
  await waitFor(GEAR_STATE, (state) => state.presentation === "collapsed");
  const beforeQuit = await reportState("before quit");
  assertEqual("collapsed state applied before quit", beforeQuit?.settings?.floatingPresentation, "collapsed");
  await quitAndWait(pid, "first");

  socket?.close();
  pid = launchApp();
  await connectToApp();
  await send("Page.enable");
  const afterRestart = await waitFor(GEAR_STATE, (state) => state.presentation !== null);
  const restartedState = await reportState("after restart");
  assertEqual("restart keeps the persisted Orb presentation", afterRestart.presentation, "collapsed");
  assertEqual("restart keeps the Orb, with no gear", afterRestart.gearCount, 0);
  assertEqual("restart renders the Orb", afterRestart.orbCount, 1);
  assertEqual("restart opened the same profile database",
    restartedState?.databasePath, beforeQuit?.databasePath);
  assertEqual("restart keeps the saved Glass material (no fresh-profile re-default)",
    restartedState?.settings?.appearanceProfiles?.[restartedState?.settings?.mode]?.backgroundType, "glass");

  // --- I. quit leaves nothing behind --------------------------------------
  console.log("quit");
  await quitAndWait(pid, "second");

  // --- J. release warnings still reach the log file ------------------------
  // A release build has no console, so anything that only used `eprintln!` would
  // be invisible. The startup mismatch guard is deliberately reachable here: the
  // early rendering-backend read follows %APPDATA% while the product database
  // follows the Windows known folder, so redirecting APPDATA makes the two
  // diverge and has to produce a log record.
  console.log("release diagnostics routing");
  const redirected = mkdtempSync(join(tmpdir(), "dtw-redirected-appdata-"));
  try {
    socket?.close();
    pid = launchApp({ APPDATA: redirected });
    await connectToApp();
    await send("Page.enable");
    const logName = readdirSync(dirname(exe)).find((name) => name.startsWith("phase7b-qa-") && name.endsWith(".log"));
    check("QA log file present after a fresh launch", Boolean(logName));
    // The setup hook writes the mismatch record after the page target exists, so
    // poll instead of reading once.
    let log = "";
    for (let attempt = 0; attempt < 20; attempt += 1) {
      log = logName ? readFileSync(join(dirname(exe), logName), "utf8") : "";
      if (log.includes("[rendering] app_data_dir_mismatch")) break;
      await sleep(500);
    }
    check("startup warnings are written to the log, not only to stderr",
      log.includes("[rendering] app_data_dir_mismatch"),
      log.split("\n").filter(Boolean).slice(-3).join(" | "));
    await quitAndWait(pid, "redirected-appdata");
  } finally {
    rmSync(redirected, { recursive: true, force: true });
  }
} finally {
  socket?.close();
  // Never leave a stray instance from a failed run behind: the wrapper has to be
  // able to delete the scratch profile and put the real one back.
  for (const stray of productPids() ?? []) {
    try { process.kill(stray); } catch { /* ignore */ }
  }
  await sleep(500);
}

const failed = checks.filter((entry) => !entry.pass);
console.log("");
console.log(`${checks.length - failed.length}/${checks.length} checks passed`);
if (failed.length > 0) {
  for (const entry of failed) console.log(`  failed: ${entry.name}`);
  process.exit(1);
}
process.exit(0);
