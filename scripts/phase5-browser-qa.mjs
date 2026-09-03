import { createRequire } from "node:module";
import { mkdir } from "node:fs/promises";

const require = createRequire(import.meta.url);
const { chromium } = require(
  "C:/Users/Alan Floyd/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright",
);

const base = "http://127.0.0.1:1420/";
const output = "C:/Users/Alan Floyd/.codex/visualizations/2026/08/31/01a055bf-e43b-7a02-929a-7eb674c2c02b";
await mkdir(output, { recursive: true });

const browser = await chromium.launch({
  headless: true,
  executablePath: "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
});
const results = [];

async function materialDiagnostics(page) {
  return page.evaluate(() => {
    const style = (selector) => {
      const element = document.querySelector(selector);
      if (!element) throw new Error(`Missing diagnostics element: ${selector}`);
      const computed = getComputedStyle(element);
      return {
        background: computed.backgroundColor,
        borderRadius: computed.borderRadius,
        clipPath: computed.clipPath,
        opacity: computed.opacity,
        backdropFilter: computed.backdropFilter,
      };
    };
    return {
      root: style(":root"),
      body: style("body"),
      app: style("#app"),
      shell: style(".app-shell"),
      material: style(".appearance-material"),
      backgroundLayer: style(".appearance-background-layer"),
      tintLayer: style(".appearance-tint-layer"),
      contentLayer: style(".appearance-content-layer"),
    };
  });
}

function assertTransparent(name, diagnostics) {
  for (const layer of ["root", "body", "app", "shell", "material"]) {
    if (diagnostics[layer].background !== "rgba(0, 0, 0, 0)") {
      throw new Error(`${name}: ${layer} is opaque: ${diagnostics[layer].background}`);
    }
  }
}

async function inspect(name, width, height, query, interact) {
  const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: 1 });
  const consoleProblems = [];
  const pageErrors = [];
  page.on("console", (message) => {
    if (["warning", "error"].includes(message.type())) {
      consoleProblems.push(`${message.type()}: ${message.text()}`);
    }
  });
  page.on("pageerror", (error) => {
    pageErrors.push(error.message);
  });
  await page.goto(`${base}?${query}`, { waitUntil: "networkidle" });
  if (interact) await interact(page);
  const metrics = await page.evaluate(() => ({
    documentWidth: document.documentElement.scrollWidth,
    bodyWidth: document.body.scrollWidth,
    viewportWidth: window.innerWidth,
    documentHeight: document.documentElement.scrollHeight,
    viewportHeight: window.innerHeight,
    weather: document.body.innerText.includes("28°C") && document.body.innerText.includes("RAIN 2%"),
  }));
  const material = await materialDiagnostics(page);
  await page.screenshot({ path: `${output}/phase5-${name}.png` });
  results.push({ name, width, height, metrics, material, consoleProblems, pageErrors });
  await page.close();
}

await inspect("orb", 56, 56, "presentation=collapsed", async (page) => {
  const orb = page.getByRole("button", { name: "Open Alan Desktop" });
  await orb.focus();
  if (!(await orb.isVisible())) throw new Error("Orb is not visible");
});

await inspect("floating-glass", 620, 720, "mode=floating&background=glass", async (page) => {
  const diagnostics = await materialDiagnostics(page);
  assertTransparent("floating glass", diagnostics);
  if (diagnostics.backgroundLayer.background !== "rgba(0, 0, 0, 0)") {
    throw new Error(`Glass source layer is not transparent: ${diagnostics.backgroundLayer.background}`);
  }
  if (diagnostics.contentLayer.opacity !== "1") {
    throw new Error(`Glass content opacity is ${diagnostics.contentLayer.opacity}`);
  }
  await page.locator("main").click({ button: "right", position: { x: 580, y: 40 } });
  const settingsButton = page.getByRole("button", { name: "Settings", exact: true });
  await settingsButton.evaluate((button) => button.click());
  await page.locator(".settings-panel").waitFor({ state: "attached", timeout: 3000 });
  if (!(await page.locator(".settings-panel").isVisible())) {
    throw new Error("Settings did not open");
  }
  await page.getByRole("button", { name: "Gradient" }).click();
  await page.getByRole("button", { name: "Reset appearance" }).click();
  await page.getByRole("button", { name: "Close settings" }).click();
});

await inspect("settings-appearance", 620, 720, "mode=floating&background=glass", async (page) => {
  await page.locator("main").click({ button: "right", position: { x: 580, y: 40 } });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByText("APPEARANCE", { exact: true }).scrollIntoViewIfNeeded();
});

await inspect("floating-solid-light", 620, 720, "mode=floating&background=solid&palette=light");
await inspect("floating-gradient", 620, 720, "mode=floating&background=gradient");
await inspect("floating-image-fallback", 620, 720, "mode=floating&background=image");
await inspect("floating-wallpaper-fallback", 620, 720, "mode=floating&background=wallpaper");
await inspect("sidebar-320", 320, 720, "mode=sidebar&background=glass");
await inspect("sidebar-380", 380, 800, "mode=sidebar&background=glass");
await inspect("desktop", 420, 700, "mode=desktop&background=glass");

const shapePage = await browser.newPage({ viewport: { width: 620, height: 720 } });
await shapePage.goto(`${base}?mode=floating&presentation=collapsed&background=glass`, { waitUntil: "networkidle" });
for (let iteration = 1; iteration <= 20; iteration += 1) {
  const collapsed = await materialDiagnostics(shapePage);
  if (collapsed.shell.clipPath === "none" || collapsed.shell.borderRadius === "0px") {
    throw new Error(`Cycle ${iteration}: collapsed Orb lost its circular shape`);
  }
  await shapePage.getByRole("button", { name: "Open Alan Desktop" }).click();
  const expanded = await materialDiagnostics(shapePage);
  for (const layer of ["shell", "material", "contentLayer"]) {
    if (expanded[layer].clipPath !== "none" || expanded[layer].borderRadius !== "0px") {
      throw new Error(`Cycle ${iteration}: expanded ${layer} retained Orb shape`);
    }
  }
  await shapePage.getByRole("button", { name: "Collapse to Avatar Orb" }).click();
}
await shapePage.close();

for (const mode of ["sidebar", "desktop"]) {
  const page = await browser.newPage({ viewport: { width: 420, height: 700 } });
  await page.goto(`${base}?mode=${mode}&presentation=collapsed&background=glass`, { waitUntil: "networkidle" });
  const diagnostics = await materialDiagnostics(page);
  for (const layer of ["shell", "material", "contentLayer"]) {
    if (diagnostics[layer].clipPath !== "none" || diagnostics[layer].borderRadius !== "0px") {
      throw new Error(`${mode}: ${layer} inherited Orb shape`);
    }
  }
  assertTransparent(`${mode} glass`, diagnostics);
  await page.close();
}

const interactionPage = await browser.newPage({ viewport: { width: 120, height: 120 } });
await interactionPage.goto(`${base}?presentation=collapsed`, { waitUntil: "networkidle" });
const interactionOrb = interactionPage.getByRole("button", { name: "Open Alan Desktop" });
const box = await interactionOrb.boundingBox();
if (!box) throw new Error("Orb has no interaction bounds");
await interactionPage.mouse.move(box.x + 20, box.y + 20);
await interactionPage.mouse.down();
await interactionPage.mouse.move(box.x + 32, box.y + 20);
await interactionPage.mouse.up();
if (!(await interactionOrb.isVisible())) throw new Error("Orb drag incorrectly expanded the Widget");
await interactionOrb.click();
if (!(await interactionPage.locator(".product-content").isVisible())) {
  throw new Error("Orb click did not expand the Widget");
}
await interactionPage.close();

await browser.close();
const failures = results.filter((result) =>
  result.consoleProblems.length ||
  result.pageErrors.length ||
  result.metrics.documentWidth > result.metrics.viewportWidth
);
console.log(JSON.stringify({ results, failures }, null, 2));
if (failures.length) process.exitCode = 1;
