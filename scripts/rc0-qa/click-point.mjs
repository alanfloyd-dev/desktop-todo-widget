// Click a point inside the product window using CDP mouse events.
//
// QA helper for menu items whose labels are non-ASCII: passing them through the
// PowerShell command line mangles them, so the caller resolves the label to page
// coordinates with `page-drive.mjs eval` and passes numbers here instead.
//
//   node scripts/rc0-qa/click-point.mjs 168 85
const [x, y] = process.argv.slice(2).map(Number);
if (!Number.isFinite(x) || !Number.isFinite(y)) {
  console.error("usage: node click-point.mjs <x> <y>");
  process.exit(2);
}

const PORT = Number(process.env.CDP_PORT ?? 9333);
const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((target) => target.type === "page" && target.url.includes("tauri.localhost"));
if (!page) throw new Error(`no product page target on port ${PORT}`);

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve);
  socket.addEventListener("error", () => reject(new Error("websocket error")));
});

let id = 0;
const send = (method, params) =>
  new Promise((resolve) => {
    const messageId = ++id;
    const onMessage = (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== messageId) return;
      socket.removeEventListener("message", onMessage);
      resolve(message.result);
    };
    socket.addEventListener("message", onMessage);
    socket.send(JSON.stringify({ id: messageId, method, params }));
  });

for (const type of ["mousePressed", "mouseReleased"]) {
  await send("Input.dispatchMouseEvent", { type, x, y, button: "left", clickCount: 1 });
}
console.log(JSON.stringify({ clicked: { x, y } }));
socket.close();
