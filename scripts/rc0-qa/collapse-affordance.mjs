// Verify the Floating collapse affordance across all four states, with the window
// mode read from the page rather than guessed from the window size.
//
//   node scripts/rc0-qa/collapse-affordance.mjs
//
// QA tooling only; nothing in the product imports it.

const PORT = Number(process.env.CDP_PORT ?? 9333);

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((target) => target.type === "page" && target.url.includes("tauri.localhost"));
if (!page) throw new Error(`no product page target on port ${PORT}`);

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve);
  socket.addEventListener("error", () => reject(new Error("websocket error")));
});

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
    setTimeout(() => reject(new Error(`${method}: timed out`)), 15000);
  });
}

async function evaluate(expression) {
  const result = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) {
    throw new Error(`evaluate failed: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
  }
  return result.result.value;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const STATE = `
(() => {
  const footer = document.querySelector('.signature');
  const collapse = footer ? [...footer.querySelectorAll('.collapse-affordance')] : [];
  return {
    mode: document.querySelector('.app-shell')?.dataset.windowMode ?? null,
    presentation: document.querySelector('.app-shell')?.dataset.floatingPresentation ?? null,
    orb: document.querySelector('.floating-orb') !== null,
    identity: footer?.querySelector('.profile-identity')?.innerText?.trim().replace(/\\n/g, ' | ') ?? null,
    collapseControls: collapse.map((b) => ({
      cls: b.className,
      standalone: b.classList.contains('collapse-affordance-standalone'),
      text: b.innerText.trim().replace(/\\n/g, ' | '),
      aria: b.getAttribute('aria-label'),
    })),
    // The page must never render a fallback label for an unconfigured profile.
    mentionsFallbackWord: ['User', '用户'].some((word) =>
      (document.body.innerText || '').split('\\n').some((line) => line.trim() === word)),
  };
})()`;

async function productAction(action) {
  return evaluate(
    `window.__TAURI_INTERNALS__.invoke('product_action', { action: ${JSON.stringify(action)} })
       .then((r) => ({ mode: r.settings.mode, presentation: r.settings.floatingPresentation }))
       .catch((e) => ({ error: String(e) }))`,
  );
}

async function setIdentity(displayName) {
  return evaluate(
    `window.__TAURI_INTERNALS__.invoke('update_product_settings', { patch: { displayName: ${JSON.stringify(displayName)} } })
       .then((r) => ({ displayName: r.settings.displayName, avatar: r.settings.avatarAssetId }))
       .catch((e) => ({ error: String(e) }))`,
  );
}

async function reload() {
  await evaluate("location.reload()");
  await sleep(6500);
}

function report(label, state, extra = {}) {
  console.log(`\n=== ${label} ===`);
  console.log(JSON.stringify({ ...state, ...extra }, null, 2));
}

const results = [];

// 1. Floating expanded with identity: the existing interaction is preserved.
await productAction("mode.floating");
await setIdentity("Alan Floyd");
await reload();
await productAction("floating.expand");
await sleep(3500);
let state = await evaluate(STATE);
report("Floating expanded + identity", state);
results.push({
  case: "floating + identity",
  collapseControls: state.collapseControls.length,
  identity: state.identity,
  standalone: state.collapseControls.some((c) => c.standalone),
});

state = await evaluate(STATE);
const identityControl = state.collapseControls[0];
if (state.collapseControls.length !== 1 || identityControl?.standalone) {
  console.error("FAIL: expected exactly the identity block as the collapse control");
} else {
  await evaluate(`document.querySelector('.signature .collapse-affordance').click()`);
  await sleep(3000);
  const after = await evaluate(STATE);
  console.log(`collapse via identity block -> presentation=${after.presentation} orb=${after.orb}`);
}

// 2. Floating expanded without identity: standalone control appears and works.
await productAction("floating.expand");
await sleep(2500);
await setIdentity("");
await reload();
await productAction("floating.expand");
await sleep(3500);
state = await evaluate(STATE);
report("Floating expanded + no identity", state);
results.push({
  case: "floating + no identity",
  collapseControls: state.collapseControls.length,
  identity: state.identity,
  standalone: state.collapseControls.some((c) => c.standalone),
  mentionsFallbackWord: state.mentionsFallbackWord,
});

if (state.identity !== null) {
  console.error("FAIL: an identity block rendered without identity content");
} else if (!state.collapseControls.some((c) => c.standalone)) {
  console.error("FAIL: no standalone collapse control");
} else {
  await evaluate(`document.querySelector('.collapse-affordance-standalone').click()`);
  await sleep(3000);
  const after = await evaluate(STATE);
  console.log(`collapse via standalone -> presentation=${after.presentation} orb=${after.orb}`);
}

// 3. Orb expands as before.
await evaluate(`(() => {
  const orb = document.querySelector('.floating-orb');
  const r = orb.getBoundingClientRect();
  const o = { bubbles: true, cancelable: true, pointerId: 1, pointerType: 'mouse', button: 0,
              clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
  orb.dispatchEvent(new PointerEvent('pointerdown', o));
  orb.dispatchEvent(new PointerEvent('pointerup', o));
  return true;
})()`);
await sleep(3500);
state = await evaluate(STATE);
report("Orb expand", state);
results.push({
  case: "orb expand",
  presentation: state.presentation,
  standalone: state.collapseControls.some((c) => c.standalone),
});

// 4. Sidebar and Desktop are unaffected: no collapse control, no identity.
for (const [action, label] of [["mode.sidebar", "Sidebar"], ["mode.desktop", "Desktop"]]) {
  await productAction(action);
  await sleep(3500);
  const modeState = await evaluate(STATE);
  report(`${label} (no identity)`, modeState);
  results.push({
    case: `${label.toLowerCase()} + no identity`,
    mode: modeState.mode,
    collapseControls: modeState.collapseControls.length,
    identity: modeState.identity,
  });
}

console.log("\n=== summary ===");
for (const row of results) {
  console.log(JSON.stringify(row));
}

// Restore Floating so the app is left in the mode it started this script in.
await productAction("mode.floating");
socket.close();
