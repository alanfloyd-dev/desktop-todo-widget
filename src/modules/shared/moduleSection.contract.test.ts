import { describe, expect, it } from "vitest";
import moduleSectionSource from "./ModuleSection.vue?raw";
import productContentSource from "../../components/ProductContent.vue?raw";
import todoPanelSource from "../todo/TodoPanel.vue?raw";
import githubPanelSource from "../github/GithubPanel.vue?raw";

/**
 * Source contracts of the collapse container (D2).
 *
 * These are the accessibility and lifecycle guarantees a plain-logic unit
 * test cannot reach without component-mounting infrastructure. They are
 * pinned against the compiled-away truth — the component source — the same
 * way `updateCheck.test.ts` pins its panel. A refactor that breaks one of
 * these lines changes behavior the native UI depends on and must revisit
 * this file deliberately.
 */
describe("ModuleSection source contracts (D2)", () => {
  it("toggles through a native button: Enter and Space activate it for free", () => {
    expect(moduleSectionSource).toContain('<button\n          type="button"');
    expect(moduleSectionSource).toContain('class="module-section-toggle"');
  });

  it("exposes the collapse state and body relationship to assistive tech", () => {
    expect(moduleSectionSource).toContain(':aria-expanded="!collapsed"');
    expect(moduleSectionSource).toContain(':aria-controls="bodyId"');
    // aria-controls must name a real element: the body carries the same id.
    expect(moduleSectionSource).toContain('<div :id="bodyId"');
  });

  it("collapses visibility-only: the body keeps every child mounted and unfocusable", () => {
    // v-show (display:none) is the load-bearing choice: children stay mounted
    // (drafts, listeners survive) and hidden content is not keyboard-focusable.
    // A v-if here would unmount the Todo panel and lose its state.
    expect(moduleSectionSource).toContain('v-show="!collapsed"');
    expect(moduleSectionSource).not.toMatch(/<div\s+:id="bodyId"[^>]*v-if/);
  });

  it("can disable the toggle when persistence is unavailable (D2)", () => {
    expect(moduleSectionSource).toContain(':disabled="toggleDisabled"');
    // The explanation replaces the action label, so the accessible name and
    // the tooltip both carry the reason while the button is disabled.
    expect(moduleSectionSource).toContain('t("modules.collapseUnavailable")');
  });
});

describe("collapse integration source contracts (D2)", () => {
  it("forwards the availability decision from the raw layout document to both toggles", () => {
    // The app derives availability from the RAW stored document; these pins
    // keep the decision flowing to the Todo panel and the Quick Links
    // section if either call site is reshaped.
    expect(productContentSource).toContain(":collapse-available=\"collapseAvailable\"");
    expect(productContentSource).toContain(":toggle-disabled=\"!collapseAvailable\"");
  });

  it("keeps the Todo panel collapsible through its own container, never remounted on toggle", () => {
    // The todo section's container lives inside TodoPanel itself (self-
    // consumed, D1): the whole panel — draft, category composer, drag
    // listeners — sits behind the v-show body, and ProductContent only
    // forwards collapsed state and availability into it.
    expect(todoPanelSource).toContain("<ModuleSection");
    expect(todoPanelSource).toContain("</ModuleSection>");
    expect(todoPanelSource).toContain(':toggle-disabled="!collapseAvailable"');
  });

  it("pairs every drag-listener registration with its removal in the Todo panel", () => {
    // Collapse relies on the panel staying mounted; these listeners must be
    // registered and removed symmetrically so repeated collapse/expand or
    // visibility cycles cannot leak window-level handlers.
    const add = todoPanelSource.match(/addEventListener/g)?.length ?? 0;
    const remove = todoPanelSource.match(/removeEventListener/g)?.length ?? 0;
    expect(add).toBeGreaterThan(0);
    expect(add).toBe(remove);
    // The Review entry lives in the panel's header and stays available as
    // designed in every collapse state of the container.
    expect(todoPanelSource).toContain("review-entry");
  });
});

describe("GitHub panel source contracts (G1)", () => {
  it("renders every interactive control as a real labeled button", () => {
    // The refresh affordance must stay keyboard-activatable with a localized
    // accessible name — an icon-only control without a label would be
    // unnamed to assistive technology.
    expect(githubPanelSource).toContain('<button\n      v-if="github.configured"');
    expect(githubPanelSource).toContain('type="button"');
    expect(githubPanelSource).toContain(':aria-label="t(\'github.refreshAriaLabel\')"');
    // The profile hand-off is a labeled control, never a raw anchor with an
    // unvalidated href: the username goes to the backend command, which
    // re-validates before the shell opens the fixed profile origin.
    expect(githubPanelSource).toContain(':aria-label="t(\'github.profileAriaLabel\'');
    expect(githubPanelSource).not.toMatch(/<a\s/i);
  });

  it("never fetches by itself: presentation only, actions emitted", () => {
    // No requests from render, hover, or collapse events (G1 request
    // policy): the card renders from the app-owned state and hands every
    // action up. Pins the boundary if the component is ever reshaped.
    expect(githubPanelSource).not.toContain("invoke");
    expect(githubPanelSource).not.toContain("fetch(");
    expect(githubPanelSource).toContain("$emit('refresh')");
    expect(githubPanelSource).toContain("$emit('configure')");
    expect(githubPanelSource).toContain("$emit('openProfile')");
  });
});
