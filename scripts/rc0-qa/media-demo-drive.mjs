// Drive the README demo sequence over CDP so a screen recording captures real
// product transitions.
//
// The widget must stay visible for the whole clip: the Acrylic compositor only
// paints a window that is on screen, so frames cannot be rendered off-screen and
// stitched later. Each step therefore drives the real UI and waits for the DOM to
// report the new state.
//
// Sequence: Floating (expanded) -> collapse to Orb -> expand -> Sidebar ->
// Desktop -> Settings.
const PORT = Number(process.env.CDP_PORT ?? 9340);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((t) => t.type === "page" && t.url.includes("tauri.localhost"));
if (!page) throw new Error("no product page target");

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  socket.addEventListener("open", res);
  socket.addEventListener("error", () => rej(new Error("ws error")));
});

let id = 0;
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const messageId = ++id;
    const onMessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== messageId) return;
      socket.removeEventListener("message", onMessage);
      if (message.error) reject(new Error(message.error.message));
      else resolve(message.result);
    };
    socket.addEventListener("message", onMessage);
    socket.send(JSON.stringify({ id: messageId, method, params }));
    setTimeout(() => reject(new Error(`${method} timeout`)), 10000);
  });

async function evaluate(expression) {
  const result = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
  return result.result.value;
}

async function state() {
  return evaluate(`JSON.stringify({
    shell: document.querySelector('.app-shell')?.className,
    settings: !!document.querySelector('.settings-panel'),
    orb: !!document.querySelector('.floating-orb'),
    win: window.innerWidth + 'x' + window.innerHeight,
  })`);
}

async function waitFor(predicate, label, attempts = 40) {
  for (let i = 0; i < attempts; i++) {
    const raw = await state();
    const value = JSON.parse(raw);
    if (predicate(value)) return value;
    await sleep(150);
  }
  throw new Error(`timed out waiting for ${label}`);
}

const CLICK_TEXT = (text) => `
(() => {
  const items = [...document.querySelectorAll('.context-menu button')];
  const button = items.find((b) => b.textContent.trim() === ${JSON.stringify(text)});
  if (!button) return 'missing:' + items.map((b) => b.textContent.trim()).join('|');
  button.click();
  return 'clicked';
})()`;

/**
 * Invoke a product action through the same IPC command the tray and context menu
 * dispatch. The menu item's DOM click is unreliable over the protocol (the menu
 * calls `stopPropagation` on pointerdown and some synthesized clicks are dropped),
 * whereas this is the identical backend entry point the item ends in.
 */
const PRODUCT_ACTION = (action) => `
(async () => {
  try {
    const view = await window.__TAURI_INTERNALS__.invoke('product_action', { action: ${JSON.stringify(action)} });
    return 'ok:' + (view?.settings?.mode ?? '?');
  } catch (error) {
    return 'error:' + String(error);
  }
})()`;

async function productAction(action) {
  return evaluate(PRODUCT_ACTION(action));
}

async function openMenu() {
  return evaluate(`(() => {
    const shell = document.querySelector('.app-shell');
    if (!shell) return 'no shell';
    shell.dispatchEvent(new MouseEvent('contextmenu', {
      bubbles: true, cancelable: true, clientX: 120, clientY: 120, button: 2,
    }));
    return 'opened';
  })()`);
}

async function menuAction(text) {
  await openMenu();
  await sleep(500);
  const result = await evaluate(CLICK_TEXT(text));
  await sleep(400);
  return result;
}

async function clickOrb() {
  return evaluate(`(() => {
    const orb = document.querySelector('.floating-orb');
    if (!orb) return 'no orb';
    const r = orb.getBoundingClientRect();
    for (const type of ['pointerdown', 'pointerup']) {
      orb.dispatchEvent(new PointerEvent(type, {
        bubbles: true, cancelable: true, pointerId: 1, pointerType: 'mouse', isPrimary: true,
        button: 0, buttons: type === 'pointerdown' ? 1 : 0,
        clientX: r.left + r.width / 2, clientY: r.top + r.height / 2,
      }));
    }
    return 'orb tapped';
  })()`);
}

const step = async (label, fn) => {
  const before = await state();
  const result = await fn();
  console.log(`[demo] ${label}: ${result} | before=${before}`);
};

console.log("[demo] start", await state());

await step("collapse to Orb", async () => {
  const r = await productAction("floating.collapse");
  await waitFor((s) => s.orb, "orb collapsed");
  return r;
});
await sleep(2500);

await step("expand", async () => {
  const r = await productAction("floating.expand");
  await waitFor((s) => !s.orb, "orb expanded");
  return r;
});
await sleep(2500);

await step("sidebar", async () => {
  const r = await productAction("mode.sidebar");
  await waitFor((s) => s.shell.includes("mode-sidebar"), "sidebar mode");
  return r;
});
await sleep(2800);

await step("desktop", async () => {
  const r = await productAction("mode.desktop");
  await waitFor((s) => s.shell.includes("mode-desktop"), "desktop mode");
  return r;
});
await sleep(2800);

await step("settings", async () => {
  const r = await productAction("settings");
  await waitFor((s) => s.settings, "settings panel");
  return r;
});
await sleep(3000);

console.log("[demo] end", await state());
socket.close();
