// Drive and inspect the product page over the WebView2 DevTools Protocol.
//
// The window is a WebView2 surface whose DOM is not exposed through UI Automation,
// and synthetic OS mouse events land at an offset inside it, so neither pointer
// automation nor accessibility automation can operate the Settings panel reliably.
// WebView2 does expose the standard DevTools Protocol, which is what this uses: it
// evaluates in the real page, dispatches real input events at the element's own
// rect, and reads state back.
//
// Usage (app must be running with --remote-debugging-port=9333):
//   node scripts/rc0-qa/page-drive.mjs probe
//   node scripts/rc0-qa/page-drive.mjs eval "<expression>"
//   node scripts/rc0-qa/page-drive.mjs click "<visible text>"
//   node scripts/rc0-qa/page-drive.mjs fill "<selector>" "<value>"
//   node scripts/rc0-qa/page-drive.mjs quicklinks
//
// This file is QA tooling only; nothing in the product imports it.

const PORT = Number(process.env.CDP_PORT ?? 9333);

async function pageSocket() {
  const response = await fetch(`http://127.0.0.1:${PORT}/json/list`);
  const targets = await response.json();
  const page = targets.find((target) => target.type === "page" && target.url.includes("tauri.localhost"));
  if (!page) throw new Error(`no product page target on port ${PORT}`);
  return page.webSocketDebuggerUrl;
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.addEventListener("open", () => resolve(socket));
    socket.addEventListener("error", (event) => reject(new Error(`websocket error: ${event.message ?? "unknown"}`)));
  });
}

let nextId = 1;
function send(socket, method, params = {}) {
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
    }, 15000);
  });
}

async function evaluate(socket, expression) {
  const result = await send(socket, "Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) {
    throw new Error(`evaluate failed: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
  }
  return result.result.value;
}

/** Centre of the first element whose text matches, plus a visibility check. */
const FIND_BY_TEXT = (text) => `
(() => {
  const wanted = ${JSON.stringify(text)};
  const matches = [...document.querySelectorAll('button, a, [role="button"], p, h1, h2, h3, span, li, div')]
    .filter((element) => (element.innerText || element.textContent || '').trim().includes(wanted));
  const visible = matches.filter((element) => {
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  });
  if (!visible.length) return null;
  // Deepest match: the innermost element containing the text is the one the
  // pointer would actually land on.
  const element = visible[visible.length - 1];
  element.scrollIntoView({ block: 'center' });
  const after = element.getBoundingClientRect();
  return {
    tag: element.tagName,
    text: (element.innerText || element.textContent || '').trim().slice(0, 40),
    x: after.left + after.width / 2,
    y: after.top + after.height / 2,
    disabled: element.disabled === true,
  };
})()`;

/** A DOM click at the element's own coordinates, so handlers run as they do for a user. */
async function clickText(socket, text) {
  const found = await evaluate(socket, FIND_BY_TEXT(text));
  if (!found) throw new Error(`no clickable element containing "${text}"`);
  if (found.disabled) throw new Error(`element "${found.text}" is disabled`);
  for (const type of ["mousePressed", "mouseReleased"]) {
    await send(socket, "Input.dispatchMouseEvent", {
      type,
      x: Math.round(found.x),
      y: Math.round(found.y),
      button: "left",
      clickCount: 1,
    });
  }
  return found;
}

/** Focus a field by its label text and set its value the way a user typing would. */
const FILL_BY_LABEL = (label, value) => `
(() => {
  const wanted = ${JSON.stringify(label)};
  const value = ${JSON.stringify(value)};
  const row = [...document.querySelectorAll('.settings-row, label')]
    .find((element) => (element.innerText || '').trim().startsWith(wanted));
  const input = row ? row.querySelector('input') : null;
  if (!input) return { ok: false, reason: 'input not found for ' + wanted };
  input.focus();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
  setter.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, label: wanted, value: input.value };
})()`;

/** A DOM right click at the element's own coordinates, so `contextmenu` handlers run. */
async function rightClickText(socket, text) {
  const found = await evaluate(socket, FIND_BY_TEXT(text));
  if (!found) throw new Error(`no element containing "${text}"`);
  const point = { x: Math.round(found.x), y: Math.round(found.y) };
  await send(socket, "Input.dispatchMouseEvent", { type: "mousePressed", ...point, button: "right", clickCount: 1 });
  await send(socket, "Input.dispatchMouseEvent", { type: "mouseReleased", ...point, button: "right", clickCount: 1 });
  // Chromium raises `contextmenu` from the platform event, which the protocol
  // reports as this separate type.
  await send(socket, "Input.dispatchMouseEvent", { type: "mousePressed", ...point, button: "right", clickCount: 1 });
  await send(socket, "Input.dispatchMouseEvent", { type: "mouseReleased", ...point, button: "right", clickCount: 1 });
  return found;
}

const QUICK_LINKS_STATE = `
(() => {
  const section = document.querySelector('.quick-links-settings');
  const editor = document.querySelector('.quick-link-editor');
  const productSection = document.querySelector('.quick-links');
  return {
    settingsRows: [...document.querySelectorAll('.quick-link-row')].map((row) => ({
      name: row.querySelector('.quick-link-fields strong')?.textContent?.trim() ?? '',
      url: row.querySelector('.quick-link-fields small')?.textContent?.trim() ?? '',
      up: row.querySelectorAll('.quick-link-actions button')[0]?.disabled === true ? 'disabled' : 'enabled',
      down: row.querySelectorAll('.quick-link-actions button')[1]?.disabled === true ? 'disabled' : 'enabled',
    })),
    emptyState: document.querySelector('.quick-links-empty')?.textContent?.trim() ?? null,
    editorOpen: editor !== null,
    addVisible: document.querySelector('.quick-link-add') !== null,
    error: section?.querySelector('.settings-error')?.textContent?.trim() ?? null,
    productSectionVisible: productSection !== null,
    productLinks: [...document.querySelectorAll('.quick-links .quick-link-name')].map((node) => node.textContent.trim()),
  };
})()`;

// The Settings panel is a modal view with no route or persistent control, and the
// in-page context menu that opens it is itself opened by a right click. For QA it
// is enough to set the panel's own visibility flag on the mounted root component;
// everything after that (Add, edit, reorder, delete, validation, Save) is driven
// through the real UI below.
const SET_SETTINGS_OPEN = (open) => `
(() => {
  const root = document.querySelector('#app')?.__vue_app__?._instance?.setupState;
  if (!root || typeof root.settingsOpen === 'undefined') {
    return { ok: false, reason: 'root component state unavailable' };
  }
  root.settingsOpen = ${open};
  root.reviewOpen = false;
  root.menu && (root.menu.open = false);
  return { ok: true, settingsOpen: root.settingsOpen };
})()`;

const [command, ...args] = process.argv.slice(2);
const url = await pageSocket();
const socket = await connect(url);

try {
  switch (command) {
    case "probe": {
      console.log(JSON.stringify(await evaluate(socket, QUICK_LINKS_STATE), null, 2));
      break;
    }
    case "quicklinks": {
      console.log(JSON.stringify(await evaluate(socket, QUICK_LINKS_STATE), null, 2));
      break;
    }
    case "open-settings": {
      console.log(JSON.stringify(await evaluate(socket, SET_SETTINGS_OPEN(true)), null, 2));
      break;
    }
    case "close-settings": {
      console.log(JSON.stringify(await evaluate(socket, SET_SETTINGS_OPEN(false)), null, 2));
      break;
    }
    case "eval": {
      console.log(JSON.stringify(await evaluate(socket, args.join(" ")), null, 2));
      break;
    }
    case "click": {
      console.log(JSON.stringify(await clickText(socket, args.join(" ")), null, 2));
      break;
    }
    case "rightclick": {
      console.log(JSON.stringify(await rightClickText(socket, args.join(" ")), null, 2));
      break;
    }
    case "fill": {
      console.log(JSON.stringify(await evaluate(socket, FILL_BY_LABEL(args[0], args[1])), null, 2));
      break;
    }
    default:
      console.error(`unknown command: ${command ?? "(none)"}`);
      process.exitCode = 2;
  }
} finally {
  socket.close();
}
