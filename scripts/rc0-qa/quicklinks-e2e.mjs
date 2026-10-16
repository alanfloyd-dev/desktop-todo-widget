// Quick Links end-to-end QA over the WebView2 DevTools Protocol.
//
// Runs the acceptance flow through the real UI: Settings -> Quick Links section
// (Add / edit / reorder / delete / validation), then Save, then reads the product
// section back. Restart the app and run `state` to check persistence.
//
//   node scripts/rc0-qa/quicklinks-e2e.mjs open
//   node scripts/rc0-qa/quicklinks-e2e.mjs add "GitHub" "https://github.com/example/repo"
//   node scripts/rc0-qa/quicklinks-e2e.mjs add-invalid "Broken" "not a url"
//   node scripts/rc0-qa/quicklinks-e2e.mjs add-scheme "Shell" "javascript:alert(1)"
//   node scripts/rc0-qa/quicklinks-e2e.mjs add-empty-name
//   node scripts/rc0-qa/quicklinks-e2e.mjs edit 2 "New name" "https://new.example/"
//   node scripts/rc0-qa/quicklinks-e2e.mjs move-up 2
//   node scripts/rc0-qa/quicklinks-e2e.mjs move-down 1
//   node scripts/rc0-qa/quicklinks-e2e.mjs delete 3
//   node scripts/rc0-qa/quicklinks-e2e.mjs save
//   node scripts/rc0-qa/quicklinks-e2e.mjs state
//
// QA tooling only; nothing in the product imports it.

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
  const result = await send(socket, "Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
  if (result.exceptionDetails) {
    throw new Error(`evaluate failed: ${result.exceptionDetails.exception?.description ?? result.exceptionDetails.text}`);
  }
  return result.result.value;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Clicks the first element matching `selector` whose own text contains `text`, at
 * that element's own coordinates.
 *
 * Targeting by selector + text rather than text alone is what keeps the link
 * editor's Save distinct from the panel footer's Save: a plain text search matches
 * both, and picking the wrong one silently closes the whole panel.
 */
async function clickIn(socket, selector, text) {
  const rect = await evaluate(
    socket,
    `(() => {
      const wanted = ${JSON.stringify(text)};
      const elements = [...document.querySelectorAll(${JSON.stringify(selector)})]
        .filter((element) => (element.innerText || element.textContent || '').trim().includes(wanted))
        .filter((element) => {
          const r = element.getBoundingClientRect();
          return r.width > 0 && r.height > 0;
        });
      const element = elements[0];
      if (!element) return null;
      element.scrollIntoView({ block: 'center' });
      const r = element.getBoundingClientRect();
      return { text: (element.innerText || element.textContent || '').trim().slice(0, 40), x: r.left + r.width / 2, y: r.top + r.height / 2 };
    })()`,
  );
  if (!rect) throw new Error(`no visible "${selector}" containing "${text}"`);
  for (const type of ["mousePressed", "mouseReleased"]) {
    await send(socket, "Input.dispatchMouseEvent", {
      type,
      x: Math.round(rect.x),
      y: Math.round(rect.y),
      button: "left",
      clickCount: 1,
    });
  }
  await sleep(300);
  return rect;
}

/** Sets an input's value through the native setter so Vue's v-model observes it. */
const setInput = (selector, index, value) => `
(() => {
  const inputs = [...document.querySelectorAll(${JSON.stringify(selector)})];
  const input = inputs[${index}];
  if (!input) return { ok: false, reason: 'input ${index} missing in ' + ${JSON.stringify(selector)} };
  input.focus();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
  setter.call(input, ${JSON.stringify(value)});
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, value: input.value };
})()`;

const STATE = `
(() => {
  const rows = [...document.querySelectorAll('.quick-link-row')];
  return {
    list: rows.map((row) => ({
      name: row.querySelector('.quick-link-fields strong')?.textContent?.trim() ?? '',
      url: row.querySelector('.quick-link-fields small')?.textContent?.trim() ?? '',
      upDisabled: row.querySelectorAll('.quick-link-actions button')[0]?.disabled === true,
      downDisabled: row.querySelectorAll('.quick-link-actions button')[1]?.disabled === true,
    })),
    settingsEmpty: document.querySelector('.quick-links-empty')?.textContent?.trim() ?? null,
    editorOpen: document.querySelector('.quick-link-editor') !== null,
    addVisible: document.querySelector('.quick-link-add') !== null,
    editorError: document.querySelector('.quick-links-settings .settings-error')?.textContent?.trim() ?? null,
    panelOpen: document.querySelector('.settings-panel') !== null,
    productVisible: document.querySelector('.quick-links') !== null,
    product: [...document.querySelectorAll('.quick-links .quick-link-name')].map((node) => node.textContent.trim()),
  };
})()`;

/** Row ordinal is 1-based in the CLI so it matches what the user counts on screen. */
function rowIndex(value) {
  const parsed = Number.parseInt(value, 10);
  if (!Number.isFinite(parsed) || parsed < 1) throw new Error(`row ordinal must be >= 1, got "${value}"`);
  return parsed - 1;
}

async function clickRowAction(socket, index, actionIndex, label) {
  const clicked = await evaluate(
    socket,
    `(() => {
      const row = document.querySelectorAll('.quick-link-row')[${index}];
      if (!row) return { ok: false, reason: 'row ${index} missing' };
      const button = row.querySelectorAll('.quick-link-actions button')[${actionIndex}];
      if (!button) return { ok: false, reason: '${label} button missing' };
      if (button.disabled) return { ok: false, reason: '${label} button disabled' };
      button.click();
      return { ok: true, row: ${index + 1}, action: '${label}' };
    })()`,
  );
  if (!clicked.ok) throw new Error(clicked.reason);
  await sleep(300);
  return clicked;
}

async function fillEditor(socket, name, url) {
  const first = await evaluate(socket, setInput(".quick-link-editor input", 0, name));
  if (!first.ok) throw new Error(first.reason);
  const second = await evaluate(socket, setInput(".quick-link-editor input", 1, url));
  if (!second.ok) throw new Error(second.reason);
}

async function openSettings(socket) {
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    const already = await evaluate(socket, `document.querySelector('.settings-panel') !== null`);
    if (already) return;
    const collapsed = await evaluate(socket, `document.querySelector('.app-shell')?.dataset.floatingPresentation === 'collapsed'`);
    if (collapsed) await clickIn(socket, ".floating-orb", "");
    const point = await evaluate(
      socket,
      `(() => {
        const line = document.querySelector('.date-line');
        const r = line.getBoundingClientRect();
        return { x: r.left + 5, y: r.top + r.height / 2 };
      })()`,
    );
    for (const type of ["mousePressed", "mouseReleased"]) {
      await send(socket, "Input.dispatchMouseEvent", {
        type,
        x: Math.round(point.x),
        y: Math.round(point.y),
        button: "right",
        clickCount: 1,
      });
    }
    await sleep(500);
    let menuOpen = await evaluate(socket, `document.querySelector('.context-menu') !== null`);
    if (!menuOpen) {
      // Fallback: the handler is a `contextmenu` listener, and the protocol's
      // button events do not always synthesize that event. Dispatching the DOM
      // event still exercises the product's own handler.
      await evaluate(
        socket,
        `(() => {
          const line = document.querySelector('.date-line');
          const r = line.getBoundingClientRect();
          line.dispatchEvent(new MouseEvent('contextmenu', {
            bubbles: true,
            cancelable: true,
            clientX: r.left + 5,
            clientY: r.top + r.height / 2,
          }));
          return true;
        })()`,
      );
      await sleep(400);
      menuOpen = await evaluate(socket, `document.querySelector('.context-menu') !== null`);
    }
    if (!menuOpen) {
      console.error(`open: context menu did not appear on attempt ${attempt}`);
      continue;
    }
    await clickIn(socket, ".context-menu button", "设置");
    await sleep(700);
  }
  const open = await evaluate(socket, `document.querySelector('.settings-panel') !== null`);
  if (!open) throw new Error("could not open the Settings panel");
}

const [command, ...args] = process.argv.slice(2);
const socket = await connect(await pageSocket());

try {
  switch (command) {
    case "open": {
      await openSettings(socket);
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "state": {
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "add": {
      await clickIn(socket, ".quick-link-add", "添加链接");
      await fillEditor(socket, args[0], args[1]);
      await clickIn(socket, ".quick-link-editor-actions button", "保存");
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "add-invalid": {
      await clickIn(socket, ".quick-link-add", "添加链接");
      await fillEditor(socket, args[0] ?? "Broken", args[1] ?? "not a url");
      await clickIn(socket, ".quick-link-editor-actions button", "保存");
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "add-scheme": {
      await clickIn(socket, ".quick-link-add", "添加链接");
      await fillEditor(socket, args[0] ?? "Shell", args[1] ?? "javascript:alert(1)");
      await clickIn(socket, ".quick-link-editor-actions button", "保存");
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "add-empty-name": {
      await clickIn(socket, ".quick-link-add", "添加链接");
      await fillEditor(socket, "   ", "https://example.com/");
      await clickIn(socket, ".quick-link-editor-actions button", "保存");
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "cancel": {
      await clickIn(socket, ".quick-link-editor-actions button", "取消");
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "edit": {
      await clickRowAction(socket, rowIndex(args[0]), 2, "edit");
      await fillEditor(socket, args[1], args[2]);
      await clickIn(socket, ".quick-link-editor-actions button", "保存");
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "move-up": {
      console.log(JSON.stringify(await clickRowAction(socket, rowIndex(args[0]), 0, "up")));
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "move-down": {
      console.log(JSON.stringify(await clickRowAction(socket, rowIndex(args[0]), 1, "down")));
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "delete": {
      console.log(JSON.stringify(await clickRowAction(socket, rowIndex(args[0]), 3, "delete")));
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    case "save": {
      await clickIn(socket, ".save-settings", "保存设置");
      await sleep(1500);
      console.log(JSON.stringify(await evaluate(socket, STATE), null, 2));
      break;
    }
    default:
      console.error(`unknown command: ${command ?? "(none)"}`);
      process.exitCode = 2;
  }
} finally {
  socket.close();
}
