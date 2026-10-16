// New-task affordance QA over the WebView2 DevTools Protocol.
//
// Verifies the v1 rule that pressing Enter in the title field and clicking the `＋`
// submit control reach the same add path, that an empty submit creates nothing,
// and that an Enter which belongs to an IME composition is not treated as a
// submit.
//
//   node scripts/rc0-qa/new-task-e2e.mjs form      # structure + accessible name
//   node scripts/rc0-qa/new-task-e2e.mjs enter     # type + Enter
//   node scripts/rc0-qa/new-task-e2e.mjs click     # type + click ＋
//   node scripts/rc0-qa/new-task-e2e.mjs empty     # click ＋ with no text
//   node scripts/rc0-qa/new-task-e2e.mjs composing # Enter during IME composition
//   node scripts/rc0-qa/new-task-e2e.mjs cleanup   # remove the tasks it created
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
    setTimeout(() => {
      socket.removeEventListener("message", onMessage);
      reject(new Error(`${method}: timed out`));
    }, 15000);
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

const FORM = `
(() => {
  const form = document.querySelector('.task-add');
  if (!form) return { form: false };
  const plus = form.querySelector('.add-submit');
  return {
    form: true,
    plusTag: plus?.tagName ?? null,
    plusType: plus?.getAttribute('type') ?? null,
    plusText: (plus?.textContent ?? '').trim(),
    plusAriaLabel: plus?.getAttribute('aria-label') ?? null,
    plusTitle: plus?.getAttribute('title') ?? null,
    submitControls: [...form.querySelectorAll('button[type="submit"], input[type="submit"]')].length,
    visibleSubmitLabels: [...form.querySelectorAll('button[type="submit"]')].map((b) => (b.textContent ?? '').trim()),
    inputAriaLabel: form.querySelector('input')?.getAttribute('aria-label') ?? null,
  };
})()`;

/** Titles currently pending, in order. */
const PENDING = `[...document.querySelectorAll('.pending-row .task-copy span')].map((n) => n.textContent.trim())`;

/** Dismisses the "previous task day" review block if it is in the way. */
const CLEAR_REVIEW = `
(() => {
  const keep = [...document.querySelectorAll('.review-row button')]
    .filter((b) => (b.textContent || '').includes('保留') || (b.textContent || '').includes('Keep'));
  keep.forEach((b) => b.click());
  return keep.length;
})()`;

/** Focuses the new-task input and returns its page coordinates. */
const FOCUS_INPUT = `
(() => {
  const input = document.querySelector('.task-add input');
  input.focus();
  const r = input.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2, focused: document.activeElement === input };
})()`;

const PLUS_POINT = `
(() => {
  const plus = document.querySelector('.task-add .add-submit');
  const r = plus.getBoundingClientRect();
  return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
})()`;

async function setTitle(value) {
  return evaluate(
    `(() => {
      const input = document.querySelector('.task-add input');
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
      setter.call(input, ${JSON.stringify(value)});
      input.dispatchEvent(new Event('input', { bubbles: true }));
      return input.value;
    })()`,
  );
}

/** A real Enter keydown on the focused input, from the protocol. */
async function pressEnter({ composing }) {
  const point = await evaluate(FOCUS_INPUT);
  // `isComposing` cannot be set through Input.dispatchKeyEvent, so the composing
  // case dispatches the DOM event the IME actually produces: keydown with
  // isComposing true and the legacy keyCode 229.
  if (composing) {
    return evaluate(
      `(() => {
        const input = document.querySelector('.task-add input');
        const event = new KeyboardEvent('keydown', {
          key: 'Enter', code: 'Enter', keyCode: 229, which: 229,
          bubbles: true, cancelable: true, isComposing: true,
        });
        Object.defineProperty(event, 'isComposing', { value: true });
        const notCancelled = input.dispatchEvent(event);
        return { dispatched: true, defaultPrevented: !notCancelled, isComposing: event.isComposing };
      })()`,
    );
  }
  await send("Input.dispatchKeyEvent", {
    type: "keyDown",
    windowsVirtualKeyCode: 13,
    nativeVirtualKeyCode: 13,
    key: "Enter",
    code: "Enter",
    text: "\r",
  });
  await send("Input.dispatchKeyEvent", {
    type: "keyUp",
    windowsVirtualKeyCode: 13,
    nativeVirtualKeyCode: 13,
    key: "Enter",
    code: "Enter",
  });
  return point;
}

async function clickPlus() {
  const point = await evaluate(PLUS_POINT);
  for (const type of ["mousePressed", "mouseReleased"]) {
    await send("Input.dispatchMouseEvent", { type, x: point.x, y: point.y, button: "left", clickCount: 1 });
  }
  await sleep(900);
  return point;
}

const [command] = process.argv.slice(2);

try {
  switch (command) {
    case "form": {
      await evaluate(CLEAR_REVIEW);
      console.log(JSON.stringify(await evaluate(FORM), null, 2));
      break;
    }
    case "enter": {
      await evaluate(CLEAR_REVIEW);
      const before = await evaluate(PENDING);
      const title = `QA enter ${Date.now()}`;
      await evaluate(FOCUS_INPUT);
      await setTitle(title);
      await pressEnter({ composing: false });
      await sleep(1200);
      const after = await evaluate(PENDING);
      console.log(JSON.stringify({
        title,
        before: before.length,
        after: after.length,
        created: after.length - before.length,
        present: after.includes(title),
        inputCleared: (await evaluate(`document.querySelector('.task-add input').value`)) === "",
        pending: after,
      }, null, 2));
      break;
    }
    case "click": {
      await evaluate(CLEAR_REVIEW);
      const before = await evaluate(PENDING);
      const title = `QA click ${Date.now()}`;
      await evaluate(FOCUS_INPUT);
      await setTitle(title);
      await clickPlus();
      const after = await evaluate(PENDING);
      console.log(JSON.stringify({
        title,
        before: before.length,
        after: after.length,
        created: after.length - before.length,
        present: after.includes(title),
        inputCleared: (await evaluate(`document.querySelector('.task-add input').value`)) === "",
        pending: after,
      }, null, 2));
      break;
    }
    case "empty": {
      await evaluate(CLEAR_REVIEW);
      await evaluate(FOCUS_INPUT);
      await setTitle("");
      const before = await evaluate(PENDING);
      await clickPlus();
      const after = await evaluate(PENDING);
      console.log(JSON.stringify({
        before: before.length,
        after: after.length,
        created: after.length - before.length,
        pending: after,
      }, null, 2));
      break;
    }
    case "composing": {
      await evaluate(CLEAR_REVIEW);
      await evaluate(FOCUS_INPUT);
      const title = `QA composing ${Date.now()}`;
      await setTitle(title);
      const before = await evaluate(PENDING);
      const event = await pressEnter({ composing: true });
      await sleep(900);
      const after = await evaluate(PENDING);
      console.log(JSON.stringify({
        title,
        before: before.length,
        after: after.length,
        created: after.length - before.length,
        defaultPrevented: event.defaultPrevented,
        // The draft must survive: the IME still owns the text.
        inputValue: await evaluate(`document.querySelector('.task-add input').value`),
        pending: after,
      }, null, 2));
      break;
    }
    case "cleanup": {
      const removed = await evaluate(
        `(() => {
          let count = 0;
          for (const row of [...document.querySelectorAll('.pending-row')]) {
            const title = row.querySelector('.task-copy span')?.textContent?.trim() ?? '';
            if (!title.startsWith('QA ')) continue;
            const del = [...row.querySelectorAll('.task-actions button')]
              .find((b) => (b.textContent || '').includes('删除') || (b.textContent || '').includes('Delete'));
            if (del) { del.click(); count += 1; }
          }
          return count;
        })()`,
      );
      await sleep(1500);
      await evaluate(`(() => { const i = document.querySelector('.task-add input'); if (i) { const s = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value').set; s.call(i, ''); i.dispatchEvent(new Event('input',{bubbles:true})); } return true; })()`);
      console.log(JSON.stringify({ removed, pending: await evaluate(PENDING) }, null, 2));
      break;
    }
    default:
      console.error(`unknown command: ${command ?? "(none)"}`);
      process.exitCode = 2;
  }
} finally {
  socket.close();
}
