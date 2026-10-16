// RC-1 lifecycle driver: exercise the real product UI over the DevTools Protocol.
//
// The product exposes a WebView2 page, and Windows synthetic OS input lands at an
// offset inside it, so the UI is driven through CDP instead: real input events at
// the element's own rect, with state read back from the live DOM. This is the
// same transport `page-drive.mjs` uses; it exists separately because the RC-1
// checks need awaited settle loops and multi-step sequences.
//
// Usage: node rc1-drive.mjs <scenario> [args...]
// The app must be running with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS set to
// --remote-debugging-port=9333 (untouched release build; QA-only, no product code).

const PORT = Number(process.env.CDP_PORT ?? 9333);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function pageSocket() {
  const response = await fetch(`http://127.0.0.1:${PORT}/json/list`);
  const targets = await response.json();
  const page = targets.find(
    (target) => target.type === "page" && target.url.includes("tauri.localhost"),
  );
  if (!page) throw new Error(`no product page target on port ${PORT}`);
  return page.webSocketDebuggerUrl;
}

function connect(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    socket.addEventListener("open", () => resolve(socket));
    socket.addEventListener("error", () => reject(new Error("websocket error")));
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
    throw new Error(
      `evaluate failed: ${
        result.exceptionDetails.exception?.description ?? result.exceptionDetails.text
      }`,
    );
  }
  return result.result.value;
}

const KEY = {
  Enter: { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13, text: "\r" },
  Escape: { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 },
  Tab: { key: "Tab", code: "Tab", windowsVirtualKeyCode: 9 },
};

async function press(socket, name, modifiers = 0) {
  const descriptor = KEY[name];
  await send(socket, "Input.dispatchKeyEvent", {
    type: "rawKeyDown",
    ...descriptor,
    modifiers,
  });
  await send(socket, "Input.dispatchKeyEvent", {
    type: "keyUp",
    key: descriptor.key,
    code: descriptor.code,
    windowsVirtualKeyCode: descriptor.windowsVirtualKeyCode,
    modifiers,
  });
}

/** Set an input's value the way typing does, then fire the events Vue listens to. */
const SET_INPUT = (selector, value) => `
(() => {
  const input = document.querySelector(${JSON.stringify(selector)});
  if (!input) return { ok: false, reason: 'input not found: ' + ${JSON.stringify(selector)} };
  input.focus();
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
  setter.call(input, ${JSON.stringify(value)});
  input.dispatchEvent(new Event('input', { bubbles: true }));
  input.dispatchEvent(new Event('change', { bubbles: true }));
  return { ok: true, value: input.value };
})()`;

const CLICK_SELECTOR = (selector) => `
(() => {
  const el = document.querySelector(${JSON.stringify(selector)});
  if (!el) return { ok: false, reason: 'not found: ' + ${JSON.stringify(selector)} };
  el.scrollIntoView({ block: 'center' });
  const rect = el.getBoundingClientRect();
  return { ok: true, x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
})()`;

async function clickSelector(socket, selector) {
  const found = await evaluate(socket, CLICK_SELECTOR(selector));
  if (!found.ok) throw new Error(found.reason);
  await send(socket, "Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: Math.round(found.x),
    y: Math.round(found.y),
    button: "left",
    clickCount: 1,
  });
  await send(socket, "Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: Math.round(found.x),
    y: Math.round(found.y),
    button: "left",
    clickCount: 1,
  });
  return found;
}

/** The live todo state as the page sees it. */
/** Resolve the five appearance-related option groups by their aria-labels. */
const GROUPS = `
(() => {
  const byLabel = {};
  for (const group of document.querySelectorAll('.appearance-options')) {
    byLabel[group.getAttribute('aria-label')] = group;
  }
  return {
    language: byLabel['语言'] ?? byLabel['Language'] ?? null,
    editMode: byLabel['正在编辑外观的窗口模式'] ?? null,
    backend: byLabel['渲染后端'] ?? null,
    background: byLabel['背景样式'] ?? null,
    contrast: byLabel['文字颜色'] ?? null,
  };
})()`;

const GROUP_STATE = `
(() => {
  const groups = ${GROUPS};
  const active = (group) => group
    ? [...group.querySelectorAll('button')].filter((b) => b.className.includes('active')).map((b) => b.textContent.trim())
    : null;
  const all = (group) => group ? [...group.querySelectorAll('button')].map((b) => b.textContent.trim()) : null;
  const material = document.querySelector('.appearance-material');
  return {
    panelOpen: !!document.querySelector('.settings-panel'),
    language: active(groups.language), languageOptions: all(groups.language),
    editMode: active(groups.editMode), editModeOptions: all(groups.editMode),
    backend: active(groups.backend),
    backgroundType: active(groups.background), backgroundOptions: all(groups.background),
    textContrast: active(groups.contrast), contrastOptions: all(groups.contrast),
    resetLabel: document.querySelector('.reset-appearance')?.textContent?.trim() ?? null,
    materialStyle: material ? material.getAttribute('style') : null,
    dataBackgroundType: material ? material.getAttribute('data-background-type') : null,
    dataTextContrast: material ? material.getAttribute('data-text-contrast') : null,
    colourInputs: [...document.querySelectorAll('.settings-panel input[type=color]')].map((input) => ({
      value: input.value,
      row: input.closest('.settings-row')?.innerText?.split('\\n')[0] ?? null,
    })),
  };
})()`;

/** The live todo state as the page sees it. */
const TODO_STATE = `
(() => {
  const rows = (selector) => [...document.querySelectorAll(selector)].map((row) => ({
    id: row.dataset.taskId ?? null,
    title: row.querySelector('.task-copy span')?.textContent?.trim() ?? '',
    category: row.querySelector('.task-copy small')?.textContent?.trim() ?? null,
    cls: row.className,
  }));
  const history = [...document.querySelectorAll('.history-row')].map((row) => ({
    mark: row.querySelector('.history-mark')?.textContent?.trim() ?? '',
    title: row.querySelector('.task-copy span')?.textContent?.trim() ?? '',
    status: row.querySelector('.task-copy small')?.textContent?.trim() ?? '',
  }));
  return {
    day: document.querySelector('.today-section')?.innerText?.split('\\n').slice(0, 3).join(' | ') ?? null,
    pending: rows('.pending-row'),
    completed: rows('.task-completed'),
    history,
    empty: document.querySelector('.todo-empty')?.textContent?.trim() ?? null,
    month: document.querySelector('.month-section')?.innerText?.replace(/\\n/g, ' | ') ?? null,
    editing: document.querySelector('.task-editing') !== null,
    editValue: document.querySelector('.task-editing input')?.value ?? null,
    draft: document.querySelector('.task-add input')?.value ?? null,
  };
})()`;

async function settle(socket, expression, predicate, attempts = 30, delay = 200) {
  let last;
  for (let i = 0; i < attempts; i++) {
    last = await evaluate(socket, expression);
    if (predicate(last)) return last;
    await sleep(delay);
  }
  return last;
}

const scenario = process.argv[2];
const arg = (index, fallback = null) => process.argv[index] ?? fallback;

const socket = await connect(await pageSocket());
try {
  switch (scenario) {
    case "state": {
      console.log(JSON.stringify(await evaluate(socket, TODO_STATE), null, 1));
      break;
    }
    case "add-enter": {
      const title = arg(3);
      await evaluate(socket, SET_INPUT(".task-add input", title));
      await press(socket, "Enter");
      const state = await settle(
        socket,
        TODO_STATE,
        (value) => value.pending.some((task) => task.title === title),
      );
      console.log(JSON.stringify({ added: state.pending.some((t) => t.title === title), state }, null, 1));
      break;
    }
    case "add-button": {
      const title = arg(3);
      await evaluate(socket, SET_INPUT(".task-add input", title));
      await clickSelector(socket, ".add-submit");
      const state = await settle(
        socket,
        TODO_STATE,
        (value) => value.pending.some((task) => task.title === title),
      );
      console.log(JSON.stringify({ added: state.pending.some((t) => t.title === title), state }, null, 1));
      break;
    }
    case "add-empty": {
      await evaluate(socket, SET_INPUT(".task-add input", "   "));
      await press(socket, "Enter");
      await sleep(900);
      const state = await evaluate(socket, TODO_STATE);
      console.log(JSON.stringify({ emptyNotCreated: state.pending.length, draft: state.draft, state }, null, 1));
      break;
    }
    case "add-composing": {
      // A CJK IME composition Enter must not submit. `addSubmitFromInput` guards on
      // `event.isComposing || keyCode === 229`, so the test dispatches exactly that
      // keydown contract at the app's own handler, then confirms a real Enter after
      // composition ends *does* submit.
      const title = arg(3);
      await evaluate(socket, SET_INPUT(".task-add input", title));
      const duringComposition = await evaluate(
        socket,
        `(() => {
          const input = document.querySelector('.task-add input');
          input.dispatchEvent(new CompositionEvent('compositionstart', { bubbles: true }));
          input.dispatchEvent(new KeyboardEvent('keydown', {
            key: 'Enter', code: 'Enter', keyCode: 229, bubbles: true, cancelable: true,
            isComposing: true,
          }));
          return { draft: input.value };
        })()`,
      );
      await sleep(900);
      const afterComposition = await evaluate(socket, TODO_STATE);
      await evaluate(
        socket,
        `(() => {
          document.querySelector('.task-add input').dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
          return true;
        })()`,
      );
      await press(socket, "Enter");
      const settled = await settle(
        socket,
        TODO_STATE,
        (value) => value.pending.some((task) => task.title === title),
      );
      console.log(
        JSON.stringify(
          {
            submittedDuringComposition: afterComposition.pending.some((t) => t.title === title),
            draftAfterComposition: duringComposition.draft,
            submittedAfterComposition: settled.pending.some((t) => t.title === title),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "complete":
    case "reopen": {
      const title = arg(3);
      const reopen = scenario === "reopen";
      const rowSelector = reopen ? ".task-completed" : ".pending-row";
      const found = await evaluate(
        socket,
        `(() => {
          const row = [...document.querySelectorAll(${JSON.stringify(rowSelector)})]
            .find((r) => r.querySelector('.task-copy span')?.textContent?.trim() === ${JSON.stringify(title)});
          if (!row) return null;
          const button = row.querySelector('.task-check');
          button.scrollIntoView({ block: 'center' });
          const rect = button.getBoundingClientRect();
          return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
        })()`,
      );
      if (!found) throw new Error(`${reopen ? "completed" : "pending"} task not found: ${title}`);
      for (const type of ["mousePressed", "mouseReleased"]) {
        await send(socket, "Input.dispatchMouseEvent", {
          type,
          x: Math.round(found.x),
          y: Math.round(found.y),
          button: "left",
          clickCount: 1,
        });
      }
      const state = await settle(socket, TODO_STATE, (value) =>
        reopen
          ? value.pending.some((task) => task.title === title)
          : value.completed.some((task) => task.title === title),
      );
      console.log(
        JSON.stringify(
          {
            [reopen ? "reopened" : "completed"]: reopen
              ? state.pending.some((t) => t.title === title)
              : state.completed.some((t) => t.title === title),
            pending: state.pending.map((t) => t.title),
            completed: state.completed.map((t) => t.title),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "click-row-action": {
      // click-row-action "<title>" "<action label>" [history]
      const title = arg(3);
      const label = arg(4);
      const inHistory = arg(5) === "history";
      const found = await evaluate(
        socket,
        `(() => {
          const selector = ${inHistory ? "'.history-row'" : "'.pending-row, .task-completed'"};
          const row = [...document.querySelectorAll(selector)]
            .find((r) => r.querySelector('.task-copy span')?.textContent?.trim() === ${JSON.stringify(title)});
          if (!row) return { ok: false, reason: 'row not found: ' + ${JSON.stringify(title)} };
          const button = [...row.querySelectorAll('.task-actions button')]
            .find((b) => b.textContent.trim() === ${JSON.stringify(label)});
          if (!button) return { ok: false, reason: 'action not found: ' + ${JSON.stringify(label)},
            available: [...row.querySelectorAll('.task-actions button')].map((b) => b.textContent.trim()) };
          button.scrollIntoView({ block: 'center' });
          const rect = button.getBoundingClientRect();
          return { ok: true, x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
        })()`,
      );
      if (!found.ok) throw new Error(`${found.reason} ${found.available ? JSON.stringify(found.available) : ""}`);
      for (const type of ["mousePressed", "mouseReleased"]) {
        await send(socket, "Input.dispatchMouseEvent", {
          type,
          x: Math.round(found.x),
          y: Math.round(found.y),
          button: "left",
          clickCount: 1,
        });
      }
      await sleep(1200);
      console.log(JSON.stringify(await evaluate(socket, TODO_STATE), null, 1));
      break;
    }
    case "edit": {
      // edit "<title>" "<new title>"  -> uses Edit then Enter to save
      const title = arg(3);
      const next = arg(4);
      const opened = await evaluate(
        socket,
        `(() => {
          const row = [...document.querySelectorAll('.pending-row')]
            .find((r) => r.querySelector('.task-copy span')?.textContent?.trim() === ${JSON.stringify(title)});
          if (!row) return { ok: false, reason: 'row not found' };
          const button = [...row.querySelectorAll('.task-actions button')].find((b) => !b.className.includes('task-check'));
          const editButton = [...row.querySelectorAll('.task-actions button')][0];
          editButton.click();
          return { ok: true, label: editButton.textContent.trim() };
        })()`,
      );
      await sleep(500);
      const editing = await evaluate(socket, TODO_STATE);
      await evaluate(socket, SET_INPUT(".task-editing input", next));
      await press(socket, "Enter");
      const state = await settle(socket, TODO_STATE, (value) =>
        value.pending.some((task) => task.title === next),
      );
      console.log(
        JSON.stringify(
          {
            openedEditor: editing.editing,
            editValue: editing.editValue,
            renamed: state.pending.some((t) => t.title === next),
            oldGone: !state.pending.some((t) => t.title === title),
            state,
          },
          null,
          1,
        ),
      );
      break;
    }
    case "edit-cancel": {
      // edit-cancel "<title>" "<typed>"  -> Esc must discard the change
      const title = arg(3);
      const typed = arg(4);
      await evaluate(
        socket,
        `(() => {
          const row = [...document.querySelectorAll('.pending-row')]
            .find((r) => r.querySelector('.task-copy span')?.textContent?.trim() === ${JSON.stringify(title)});
          if (!row) return false;
          row.querySelector('.task-actions button').click();
          return true;
        })()`,
      );
      await sleep(500);
      await evaluate(socket, SET_INPUT(".task-editing input", typed));
      await press(socket, "Escape");
      await sleep(1200);
      const state = await evaluate(socket, TODO_STATE);
      console.log(
        JSON.stringify(
          {
            editorClosed: !state.editing,
            originalKept: state.pending.some((t) => t.title === title),
            typedNotSaved: !state.pending.some((t) => t.title === typed),
            state,
          },
          null,
          1,
        ),
      );
      break;
    }
    case "reorder": {
      // reorder "<from title>" "<to title>" using the drag handle with real pointer events
      const from = arg(3);
      const to = arg(4);
      const geometry = await evaluate(
        socket,
        `(() => {
          const rows = [...document.querySelectorAll('.pending-row')];
          const src = rows.find((r) => r.querySelector('.task-copy span')?.textContent?.trim() === ${JSON.stringify(from)});
          const dst = rows.find((r) => r.querySelector('.task-copy span')?.textContent?.trim() === ${JSON.stringify(to)});
          if (!src || !dst) return { ok: false, reason: 'row missing', have: rows.map((r) => r.querySelector('.task-copy span')?.textContent?.trim()) };
          const handle = src.querySelector('.task-drag-handle');
          const h = handle.getBoundingClientRect();
          const d = dst.getBoundingClientRect();
          return { ok: true,
            handle: { x: h.left + h.width / 2, y: h.top + h.height / 2 },
            target: { x: d.left + d.width / 2, y: d.top + d.height / 2 } };
        })()`,
      );
      if (!geometry.ok) throw new Error(`${geometry.reason} ${JSON.stringify(geometry.have)}`);
      const hx = Math.round(geometry.handle.x);
      const hy = Math.round(geometry.handle.y);
      const tx = Math.round(geometry.target.x);
      const ty = Math.round(geometry.target.y);
      await send(socket, "Input.dispatchMouseEvent", { type: "mousePressed", x: hx, y: hy, button: "left", clickCount: 1 });
      for (let step = 1; step <= 6; step++) {
        await send(socket, "Input.dispatchMouseEvent", {
          type: "mouseMoved",
          x: Math.round(hx + ((tx - hx) * step) / 6),
          y: Math.round(hy + ((ty - hy) * step) / 6),
          button: "left",
          buttons: 1,
        });
        await sleep(80);
      }
      await send(socket, "Input.dispatchMouseEvent", { type: "mouseReleased", x: tx, y: ty, button: "left", clickCount: 1 });
      const state = await settle(socket, TODO_STATE, (value) => {
        const first = value.pending.findIndex((task) => task.title === from);
        const second = value.pending.findIndex((task) => task.title === to);
        return first > second && second >= 0;
      });
      console.log(
        JSON.stringify(
          {
            order: state.pending.map((t) => t.title),
            fromIndex: state.pending.findIndex((t) => t.title === from),
            toIndex: state.pending.findIndex((t) => t.title === to),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "open-settings": {
      await evaluate(
        socket,
        `(() => {
          const shell = document.querySelector('.app-shell');
          shell.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 200, clientY: 200, button: 2 }));
          return true;
        })()`,
      );
      await sleep(600);
      const opened = await evaluate(
        socket,
        `(() => {
          const button = [...document.querySelectorAll('.context-menu button')]
            .find((b) => b.textContent.trim().includes('设置') || b.textContent.trim() === 'Settings');
          if (!button) return { ok: false, items: [...document.querySelectorAll('.context-menu button')].map((b) => b.textContent.trim()) };
          button.click();
          return { ok: true };
        })()`,
      );
      await settle(socket, `!!document.querySelector('.settings-panel')`, (v) => v === true, 20, 200);
      const panel = await evaluate(socket, `!!document.querySelector('.settings-panel')`);
      console.log(JSON.stringify({ opened, panel }, null, 1));
      break;
    }
    case "appearance-state": {
      console.log(JSON.stringify(await evaluate(socket, GROUP_STATE), null, 1));
      break;
    }
    case "appearance-set-profile": {
      // appearance-set-profile [mode] [background] [contrast] — "-" skips a field
      const [mode, background, contrast] = [arg(3), arg(4), arg(5)];
      const chosen = await evaluate(
        socket,
        `(() => {
          const groups = ${GROUPS};
          const pick = (group, label) => {
            if (!group || !label || label === '-') return 'skipped';
            const button = [...group.querySelectorAll('button')]
              .find((b) => b.textContent.trim().startsWith(label));
            if (!button) return 'not-found: ' + label;
            button.click();
            return 'clicked';
          };
          return {
            mode: pick(groups.editMode, ${JSON.stringify(mode)}),
            background: pick(groups.background, ${JSON.stringify(background)}),
            contrast: pick(groups.contrast, ${JSON.stringify(contrast)}),
          };
        })()`,
      );
      await sleep(500);
      console.log(JSON.stringify({ chosen, active: await evaluate(socket, GROUP_STATE) }, null, 1));
      break;
    }
    case "appearance-set-color": {
      // appearance-set-color <row label> <value>
      const result = await evaluate(
        socket,
        `(() => {
          const row = [...document.querySelectorAll('.settings-panel .settings-row')]
            .find((r) => (r.innerText || '').split('\\n')[0].trim().startsWith(${JSON.stringify(arg(3))}));
          const input = row?.querySelector('input[type=color]');
          if (!input) return { ok: false, reason: 'colour input not found for ' + ${JSON.stringify(arg(3))} };
          input.focus();
          const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
          setter.call(input, ${JSON.stringify(arg(4))});
          input.dispatchEvent(new Event('input', { bubbles: true }));
          input.dispatchEvent(new Event('change', { bubbles: true }));
          return { ok: true, row: ${JSON.stringify(arg(3))}, value: input.value };
        })()`,
      );
      await sleep(300);
      console.log(JSON.stringify(result, null, 1));
      break;
    }
    case "appearance-reset": {
      const before = await evaluate(socket, GROUP_STATE);
      await clickSelector(socket, ".reset-appearance");
      await sleep(600);
      const after = await evaluate(socket, GROUP_STATE);
      console.log(
        JSON.stringify(
          {
            before: { mode: before.editMode, background: before.backgroundType, contrast: before.textContrast },
            after: { mode: after.editMode, background: after.backgroundType, contrast: after.textContrast },
          },
          null,
          1,
        ),
      );
      break;
    }
    case "settings-save": {
      const clicked = await evaluate(
        socket,
        `(() => {
          const button = [...document.querySelectorAll('.settings-panel button')]
            .find((b) => /^(保存|Save)/.test(b.textContent.trim()));
          if (!button) return { ok: false, buttons: [...document.querySelectorAll('.settings-panel button')].map((b) => b.textContent.trim()).slice(-8) };
          button.click();
          return { ok: true, label: button.textContent.trim() };
        })()`,
      );
      await sleep(1500);
      const panelOpen = await evaluate(socket, `!!document.querySelector('.settings-panel')`);
      console.log(JSON.stringify({ clicked, panelOpen }, null, 1));
      break;
    }
    case "set-language": {
      // set-language <跟随系统|English|简体中文>
      const chosen = await evaluate(
        socket,
        `(() => {
          const group = (${GROUPS}).language;
          const button = [...(group?.querySelectorAll('button') ?? [])]
            .find((b) => b.textContent.trim() === ${JSON.stringify(arg(3))});
          if (!button) return { ok: false, available: [...(group?.querySelectorAll('button') ?? [])].map((b) => b.textContent.trim()) };
          button.click();
          return { ok: true };
        })()`,
      );
      await sleep(800);
      console.log(
        JSON.stringify(
          { chosen, body: await evaluate(socket, `document.body.innerText.slice(0, 240)`) },
          null,
          1,
        ),
      );
      break;
    }
    case "quicklinks-add": {
      // quicklinks-add <name> <url> — click "Add link", fill the inline editor, Save.
      const [name, url] = [arg(3), arg(4)];
      const opened = await evaluate(
        socket,
        `(() => {
          const button = document.querySelector('.quick-link-add');
          if (!button) return { ok: false, reason: 'add button missing' };
          button.click();
          return { ok: true };
        })()`,
      );
      await settle(socket, `!!document.querySelector('.quick-link-editor')`, (v) => v === true, 20, 200);
      const inputs = await evaluate(
        socket,
        `(() => {
          const editor = document.querySelector('.quick-link-editor');
          if (!editor) return null;
          const fields = [...editor.querySelectorAll('input')];
          return fields.map((f) => ({ type: f.type, placeholder: f.placeholder }));
        })()`,
      );
      await evaluate(
        socket,
        `(() => {
          const fields = [...document.querySelectorAll('.quick-link-editor input')];
          const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
          const values = [${JSON.stringify(name)}, ${JSON.stringify(url)}];
          fields.forEach((field, index) => {
            setter.call(field, values[index] ?? '');
            field.dispatchEvent(new Event('input', { bubbles: true }));
            field.dispatchEvent(new Event('change', { bubbles: true }));
          });
          const save = [...document.querySelectorAll('.quick-link-editor-actions button')]
            .find((b) => /保存|Save/.test(b.textContent.trim()));
          if (save) save.click();
          return true;
        })()`,
      );
      await sleep(1200);
      console.log(
        JSON.stringify(
          {
            opened,
            inputs,
            rows: await evaluate(
              socket,
              `[...document.querySelectorAll('.quick-link-row')].map((r) => ({
                name: r.querySelector('.quick-link-fields strong')?.textContent?.trim(),
                url: r.querySelector('.quick-link-fields small')?.textContent?.trim(),
              }))`,
            ),
            error: await evaluate(
              socket,
              `document.querySelector('.quick-links-settings .settings-error')?.textContent?.trim() ?? null`,
            ),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "quicklinks-action": {
      // quicklinks-action <name> <编辑|删除|↑|↓>
      const target = arg(3);
      const label = arg(4);
      const clicked = await evaluate(
        socket,
        `(() => {
          const row = [...document.querySelectorAll('.quick-link-row')].find(
            (r) => r.querySelector('.quick-link-fields strong')?.textContent?.trim() === ${JSON.stringify(target)},
          );
          if (!row) return { ok: false, reason: 'row missing', rows: [...document.querySelectorAll('.quick-link-fields strong')].map((n) => n.textContent.trim()) };
          const action = [...row.querySelectorAll('.quick-link-actions button')].find((b) => {
            const text = b.textContent.trim();
            const aria = b.getAttribute('aria-label') ?? '';
            return text === ${JSON.stringify(label)} || aria.endsWith(${JSON.stringify(label)});
          });
          if (!action) return { ok: false, reason: 'action missing', actions: [...row.querySelectorAll('.quick-link-actions button')].map((b) => b.textContent.trim()) };
          if (action.disabled) return { ok: false, reason: 'action disabled' };
          action.click();
          return { ok: true };
        })()`,
      );
      await sleep(1000);
      console.log(
        JSON.stringify(
          {
            clicked,
            rows: await evaluate(
              socket,
              `[...document.querySelectorAll('.quick-link-row')].map((r) => r.querySelector('.quick-link-fields strong')?.textContent?.trim())`,
            ),
            editorOpen: await evaluate(socket, `!!document.querySelector('.quick-link-editor')`),
            editorValues: await evaluate(
              socket,
              `[...document.querySelectorAll('.quick-link-editor input')].map((i) => i.value)`,
            ),
            error: await evaluate(
              socket,
              `document.querySelector('.quick-links-settings .settings-error')?.textContent?.trim() ?? null`,
            ),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "quicklinks-edit-save": {
      // quicklinks-edit-save <new name> <new url>
      const [name, url] = [arg(3), arg(4)];
      await evaluate(
        socket,
        `(() => {
          const fields = [...document.querySelectorAll('.quick-link-editor input')];
          const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
          const values = [${JSON.stringify(name)}, ${JSON.stringify(url)}];
          fields.forEach((field, index) => {
            setter.call(field, values[index] ?? '');
            field.dispatchEvent(new Event('input', { bubbles: true }));
            field.dispatchEvent(new Event('change', { bubbles: true }));
          });
          const save = [...document.querySelectorAll('.quick-link-editor-actions button')]
            .find((b) => /保存|Save/.test(b.textContent.trim()));
          if (save) save.click();
          return true;
        })()`,
      );
      await sleep(1200);
      console.log(
        JSON.stringify(
          {
            rows: await evaluate(
              socket,
              `[...document.querySelectorAll('.quick-link-row')].map((r) => ({
                name: r.querySelector('.quick-link-fields strong')?.textContent?.trim(),
                url: r.querySelector('.quick-link-fields small')?.textContent?.trim(),
              }))`,
            ),
            error: await evaluate(
              socket,
              `document.querySelector('.quick-links-settings .settings-error')?.textContent?.trim() ?? null`,
            ),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "quicklinks-state": {
      console.log(
        JSON.stringify(
          await evaluate(
            socket,
            `(() => ({
              rows: [...document.querySelectorAll('.quick-link-row')].map((r) => ({
                name: r.querySelector('.quick-link-fields strong')?.textContent?.trim(),
                url: r.querySelector('.quick-link-fields small')?.textContent?.trim(),
                up: !r.querySelectorAll('.quick-link-actions button')[0].disabled,
                down: !r.querySelectorAll('.quick-link-actions button')[1].disabled,
              })),
              productSection: document.querySelector('.quick-links') !== null,
              productLinks: [...document.querySelectorAll('.quick-links .quick-link-name, .quick-links a')].map((n) => n.textContent.trim()),
              empty: document.querySelector('.quick-links-empty')?.textContent?.trim() ?? null,
            }))()`,
          ),
          null,
          1,
        ),
      );
      break;
    }
    case "review-open": {
      const clicked = await evaluate(
        socket,
        `(() => {
          const button = document.querySelector('.review-entry');
          if (!button) return { ok: false, reason: 'review entry missing' };
          button.click();
          return { ok: true };
        })()`,
      );
      await sleep(1200);
      console.log(
        JSON.stringify(
          {
            clicked,
            reviewOpen: await evaluate(socket, `!!document.querySelector('.review-panel')`),
            text: await evaluate(socket, `document.querySelector('.review-panel')?.innerText?.slice(0, 600) ?? null`),
            controls: await evaluate(
              socket,
              `[...document.querySelectorAll('.review-panel button')].map((b) => b.textContent.trim()).slice(0, 12)`,
            ),
          },
          null,
          1,
        ),
      );
      break;
    }
    case "raw-eval": {
      console.log(JSON.stringify(await evaluate(socket, arg(3)), null, 1));
      break;
    }
    default:
      console.error(`unknown scenario: ${scenario}`);
      process.exitCode = 2;
  }
} finally {
  socket.close();
}
