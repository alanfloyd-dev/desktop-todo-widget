import { describe, expect, it } from "vitest";
import { MODULE_REGISTRY, type ModuleId } from "./registry";
import {
  DASHBOARD_MODULE_IDS,
  DEFAULT_MODULE_LAYOUT,
  canPersistModuleCollapse,
  hasColumnSpan,
  resolveDashboardLayout,
  resolveModuleLayout,
  setModuleCollapsed,
  type LayoutEntry,
} from "./layout";

/** Entries as loose `[id?, order?, colSpan?]` tuples: the fixtures
 * deliberately include malformed rows, so every slot stays `unknown`. */
function storedLayout(
  entries: Array<[unknown, unknown?, unknown?]>,
  version: unknown = 1,
): unknown {
  return {
    version,
    entries: entries.map(([id, order, colSpan]) => {
      const entry: Record<string, unknown> = { id };
      if (order !== undefined) entry.order = order;
      if (colSpan !== undefined) entry.colSpan = colSpan;
      return entry;
    }),
  };
}

function idsOf(layout: { entries: ReadonlyArray<{ id: unknown }> }): string[] {
  return layout.entries.map((entry) => String(entry.id));
}

describe("dashboard id projection", () => {
  it("matches the registry's dashboard entries as a set", () => {
    // Set equality, deliberately not order: the registry owns module
    // identity, while the projection's sequence is the default presentation
    // order and is free to differ from registry declaration order.
    const registryDashboardIds = MODULE_REGISTRY
      .filter((module) => module.surface === "dashboard")
      .map((module) => module.id);
    expect([...DASHBOARD_MODULE_IDS].sort()).toEqual([...registryDashboardIds].sort());
  });
});

describe("default module layout", () => {
  it("resolves absent and null preferences to the canonical default", () => {
    expect(resolveModuleLayout(undefined)).toEqual(DEFAULT_MODULE_LAYOUT);
    expect(resolveModuleLayout(null)).toEqual(DEFAULT_MODULE_LAYOUT);
  });

  it("contains exactly the known dashboard modules, review excluded", () => {
    expect(idsOf(DEFAULT_MODULE_LAYOUT).sort()).toEqual(["shortcuts", "todo", "weather"]);
    expect(idsOf(DEFAULT_MODULE_LAYOUT)).not.toContain("review");
  });

  it("reproduces the current presentation order: weather, todo, shortcuts", () => {
    expect(idsOf(DEFAULT_MODULE_LAYOUT)).toEqual(["weather", "todo", "shortcuts"]);
    expect(DEFAULT_MODULE_LAYOUT.entries.map((entry) => entry.colSpan)).toEqual([1, 1, 1]);
  });
});

describe("resolveModuleLayout", () => {
  it("keeps valid custom entries in their intended ordering and spans", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["shortcuts", 0, 2],
        ["todo", 1, 1],
        ["weather", 2, 1],
      ]),
    );
    expect(idsOf(layout)).toEqual(["shortcuts", "todo", "weather"]);
    expect(layout.entries[0]).toEqual({ id: "shortcuts", order: 0, colSpan: 2 });
  });

  it("appends missing known modules in default presentation order", () => {
    const layout = resolveModuleLayout(storedLayout([["shortcuts", 5]]));
    expect(idsOf(layout)).toEqual(["shortcuts", "weather", "todo"]);
    expect(layout.entries.map((entry) => entry.order)).toEqual([0, 1, 2]);
  });

  it("resolves duplicate known ids deterministically, first occurrence winning", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["todo", 2, 2],
        ["todo", 0, 1],
        ["weather", 1],
      ]),
    );
    expect(idsOf(layout)).toEqual(["weather", "todo", "shortcuts"]);
    expect(layout.entries.find((entry) => entry.id === "todo")?.colSpan).toBe(2);
  });

  it("normalizes invalid order values as unkeyed instead of rejecting them", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["todo", Number.NaN],
        ["weather", 1],
        ["shortcuts"],
      ]),
    );
    // weather is keyed; todo and shortcuts are unkeyed, in input sequence.
    expect(idsOf(layout)).toEqual(["weather", "todo", "shortcuts"]);
  });

  it("falls back invalid column spans to one", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["todo", 0, 3],
        ["weather", 1, 0],
        ["shortcuts", 2, "wide" as unknown as number],
      ]),
    );
    expect(layout.entries.map((entry) => entry.colSpan)).toEqual([1, 1, 1]);
  });

  it("excludes unknown module ids and malformed entries from the effective layout", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["github", 0],
        ["review", 1],
        ["todo", 2],
        [42],
        [null],
        [{ id: "weather", order: 3 }],
      ]),
    );
    // github is unknown today, review is an overlay; both never render from
    // layout. The malformed rows are skipped; the object entry survives.
    expect(idsOf(layout)).toEqual(["todo", "weather", "shortcuts"]);
  });

  it("resolves unsupported or missing layout versions to the default", () => {
    for (const version of [0, 2, "1", null]) {
      const layout = resolveModuleLayout(
        storedLayout([["shortcuts", 0]], version),
      );
      expect(layout).toEqual(DEFAULT_MODULE_LAYOUT);
    }
    // The key itself absent (a document shape C2 must tolerate) and an empty
    // object both resolve to the default rather than throwing.
    expect(resolveModuleLayout({ entries: [["shortcuts", 0]] })).toEqual(DEFAULT_MODULE_LAYOUT);
    expect(resolveModuleLayout({})).toEqual(DEFAULT_MODULE_LAYOUT);
  });

  it("ignores visibility-shaped state: layout entries carry no enabled flags", () => {
    const visibilityShaped = {
      version: 1,
      entries: [{ id: "todo", order: 0, colSpan: 1, enabled: false }],
    };
    const layout = resolveModuleLayout(visibilityShaped);
    expect(layout.entries[0]).toEqual({ id: "todo", order: 0, colSpan: 1 });
    expect("enabled" in layout.entries[0]).toBe(false);
  });

  it("keeps equal-order ties stable in input sequence", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["shortcuts", 1],
        ["weather", 1],
        ["todo", 1],
      ]),
    );
    expect(idsOf(layout)).toEqual(["shortcuts", "weather", "todo"]);
  });

  it("never mutates its input, including unknown fields on known entries", () => {
    const input = storedLayout([["todo", 0, 1]]);
    (input as { entries: Array<Record<string, unknown>> }).entries[0].futureField = {
      nested: true,
    };
    const snapshot = JSON.stringify(input);
    resolveModuleLayout(input);
    expect(JSON.stringify(input)).toBe(snapshot);
  });

  it("returns freshly allocated layouts and entries per call", () => {
    const first = resolveModuleLayout();
    const second = resolveModuleLayout();
    expect(first).not.toBe(DEFAULT_MODULE_LAYOUT);
    expect(first).not.toBe(second);
    expect(first.entries).not.toBe(second.entries);
    first.entries.forEach((entry, index) => {
      expect(entry).not.toBe(second.entries[index]);
    });
    const mutated: LayoutEntry = first.entries[0];
    mutated.order = 99;
    expect(resolveModuleLayout().entries[0].order).toBe(0);
  });
});

describe("hasColumnSpan", () => {
  it("keeps the all-ones default layout single-column on any container width", () => {
    expect(hasColumnSpan(DEFAULT_MODULE_LAYOUT)).toBe(false);
    expect(
      hasColumnSpan(
        resolveModuleLayout(storedLayout([
          ["todo", 0, 1],
          ["weather", 1, 1],
        ])),
      ),
    ).toBe(false);
  });

  it("activates two columns only when an entry requests a span of two", () => {
    expect(
      hasColumnSpan(
        resolveModuleLayout(storedLayout([
          ["weather", 0, 2],
          ["todo", 1, 1],
        ])),
      ),
    ).toBe(true);
    // A stored span that C1 normalized away (e.g. colSpan 3 → 1) does not activate.
    expect(
      hasColumnSpan(
        resolveModuleLayout(storedLayout([
          ["weather", 0, 3],
          ["todo", 1, 1],
        ])),
      ),
    ).toBe(false);
  });

  it("is independent of the collapsed state (D1)", () => {
    const collapsed = resolveModuleLayout({
      version: 1,
      entries: [
        { id: "weather", order: 0, colSpan: 2, collapsed: true },
        { id: "todo", order: 1, colSpan: 1, collapsed: true },
      ],
    });
    expect(hasColumnSpan(collapsed)).toBe(true);
    // Collapsing the only spanned module must not deactivate the two-column
    // canvas: the span preference and the collapse preference are independent.
    const collapsedSpan = resolveModuleLayout({
      version: 1,
      entries: [{ id: "weather", order: 0, colSpan: 2, collapsed: true }],
    });
    expect(hasColumnSpan(collapsedSpan)).toBe(true);
  });
});

describe("collapsed state in the resolved layout (D1)", () => {
  it("resolves entries without a collapsed member to expanded", () => {
    const layout = resolveModuleLayout(
      storedLayout([
        ["weather", 0, 1],
        ["todo", 1, 1],
      ]),
    );
    for (const entry of layout.entries) {
      expect(entry.collapsed).toBeUndefined();
    }
    // The default layout stays exactly the shape it always had.
    expect(DEFAULT_MODULE_LAYOUT.entries.every((entry) => entry.collapsed === undefined)).toBe(
      true,
    );
  });

  it("keeps a stored collapsed flag through normalization, first occurrence winning", () => {
    const layout = resolveModuleLayout({
      version: 1,
      entries: [
        { id: "todo", order: 1, colSpan: 1, collapsed: true },
        { id: "todo", order: 0, colSpan: 1, collapsed: false },
        { id: "shortcuts", order: 2, colSpan: 1, collapsed: true },
      ],
    });
    expect(layout.entries.find((entry) => entry.id === "todo")?.collapsed).toBe(true);
    expect(layout.entries.find((entry) => entry.id === "shortcuts")?.collapsed).toBe(true);
  });

  it("falls back non-boolean and false collapsed values to expanded", () => {
    const layout = resolveModuleLayout({
      version: 1,
      entries: [
        { id: "todo", order: 0, colSpan: 1, collapsed: "yes" },
        { id: "shortcuts", order: 1, colSpan: 1, collapsed: false },
        { id: "weather", order: 2, colSpan: 1, collapsed: null },
      ],
    });
    for (const entry of layout.entries) {
      expect(entry.collapsed).toBeUndefined();
    }
  });
});

describe("setModuleCollapsed (D1 raw-document patch)", () => {
  /** A raw stored document with unknown members at every level. */
  function rawDocument() {
    return {
      version: 1,
      futureRoot: { mode: "something-new" },
      entries: [
        { id: "github", order: 0, colSpan: 1, pinned: true },
        { id: "weather", order: 1, colSpan: 2, note: { deep: [1, 2] } },
        { id: "todo", order: 2, colSpan: 1, futureEntryField: "kept" },
      ],
    };
  }

  it("toggles only the target entry's collapsed member, preserving the raw document", () => {
    const raw = rawDocument();
    const patch = setModuleCollapsed(raw, "todo", true);
    expect(patch).not.toBeNull();
    // The whole raw document, value for value — unknown root field, unknown
    // entry, unknown per-entry fields — with exactly one change.
    expect(patch).toEqual({
      version: 1,
      futureRoot: { mode: "something-new" },
      entries: [
        { id: "github", order: 0, colSpan: 1, pinned: true },
        { id: "weather", order: 1, colSpan: 2, note: { deep: [1, 2] } },
        { id: "todo", order: 2, colSpan: 1, futureEntryField: "kept", collapsed: true },
      ],
    });
  });

  it("does not change the target entry's order or colSpan", () => {
    const patch = setModuleCollapsed(rawDocument(), "weather", true);
    const weather = patch?.entries.find((entry) => entry.id === "weather");
    expect(weather?.order).toBe(1);
    expect(weather?.colSpan).toBe(2);
  });

  it("never mutates its input", () => {
    const raw = rawDocument();
    setModuleCollapsed(raw, "todo", true);
    expect(raw).toEqual(rawDocument());
    expect(
      (raw.entries.find((entry) => entry.id === "todo") as Record<string, unknown>).collapsed,
    ).toBeUndefined();
  });

  it("toggling back to expanded writes an explicit false", () => {
    const collapsed = setModuleCollapsed(rawDocument(), "todo", true);
    const expanded = setModuleCollapsed(collapsed, "todo", false);
    const todo = expanded?.entries.find((entry) => entry.id === "todo");
    expect(todo?.collapsed).toBe(false);
  });

  it("appends a complete entry for a missing known module, without touching unrelated entries", () => {
    const partial = {
      version: 1,
      entries: [{ id: "shortcuts", order: 0, colSpan: 1 }],
    };
    const patch = setModuleCollapsed(partial, "todo", true);
    // The resolver places the missing todo after the stored shortcuts entry
    // and the also-missing weather module (projection order); the appended
    // entry carries that resolved position and a full shape — never a partial
    // `{ id, collapsed }`, which the backend merge would treat as erasing the
    // entry's span.
    expect(patch).toEqual({
      version: 1,
      entries: [
        { id: "shortcuts", order: 0, colSpan: 1 },
        { id: "todo", order: 2, colSpan: 1, collapsed: true },
      ],
    });
  });

  it("builds a complete document from a missing stored layout", () => {
    // Nothing stored is the generalization of the missing-entry rule: there
    // is provably nothing to preserve, so the patch is the complete default
    // layout with exactly one collapse change.
    for (const missing of [undefined, null]) {
      const patch = setModuleCollapsed(missing, "shortcuts", true);
      expect(patch).toEqual({
        version: 1,
        entries: [
          { id: "weather", order: 0, colSpan: 1 },
          { id: "todo", order: 1, colSpan: 1 },
          { id: "shortcuts", order: 2, colSpan: 1, collapsed: true },
        ],
      });
    }
  });

  it("rejects non-version-1 documents: no patch is built for an opaque layout", () => {
    const future = { version: 2, entries: [], futureField: { mode: "something-new" } };
    expect(setModuleCollapsed(future, "todo", true)).toBeNull();
    expect(setModuleCollapsed({ entries: [] }, "todo", true)).toBeNull();
    expect(setModuleCollapsed("not-an-object", "todo", true)).toBeNull();
    // A v1-shaped header over a non-array entries list is corruption, not a
    // mergeable document; refusing it keeps the fail-closed shape gate.
    expect(setModuleCollapsed({ version: 1, entries: "junk" }, "todo", true)).toBeNull();
  });

  it("toggles the first occurrence of a duplicated known id and preserves the rest", () => {
    const duplicate = {
      version: 1,
      entries: [
        { id: "todo", order: 0, colSpan: 2 },
        { id: "todo", order: 1, colSpan: 1 },
      ],
    };
    const patch = setModuleCollapsed(duplicate, "todo", true);
    expect(patch).toEqual({
      version: 1,
      entries: [
        { id: "todo", order: 0, colSpan: 2, collapsed: true },
        { id: "todo", order: 1, colSpan: 1 },
      ],
    });
  });
});

describe("resolveDashboardLayout with a registry projection fixture", () => {
  it("appends a future module registered later, without rewriting saved entries", () => {
    // A future version adds `github` to the registry; an old stored layout
    // cannot contain it. The projection form exercises that path without
    // adding the module today (the cast is the fixture, as with the bridge
    // test doubles).
    const futureDashboardIds = ["todo", "weather", "shortcuts", "github"] as ModuleId[];
    const layout = resolveDashboardLayout(futureDashboardIds, storedLayout([["todo", 4]]));
    expect(idsOf(layout)).toEqual(["todo", "weather", "shortcuts", "github"]);
    expect(layout.entries.map((entry) => entry.order)).toEqual([0, 1, 2, 3]);
    expect(layout.entries[3].colSpan).toBe(1);
  });
});

describe("canPersistModuleCollapse (D2 opaque-layout availability)", () => {
  it("accepts a provably version-1 raw document", () => {
    expect(
      canPersistModuleCollapse({
        version: 1,
        futureRoot: { kept: true },
        entries: [{ id: "todo", order: 0, colSpan: 1 }],
      }),
    ).toBe(true);
  });

  it("accepts a missing layout: there is provably nothing to preserve", () => {
    for (const missing of [undefined, null]) {
      expect(canPersistModuleCollapse(missing)).toBe(true);
    }
  });

  it("rejects every non-V1 shape setModuleCollapsed would refuse", () => {
    // A future format the build must never overwrite…
    expect(canPersistModuleCollapse({ version: 2, entries: [] })).toBe(false);
    // …a document with no version…
    expect(canPersistModuleCollapse({ entries: [] })).toBe(false);
    // …a v1-shaped header over a corrupt entries list…
    expect(canPersistModuleCollapse({ version: 1, entries: "junk" })).toBe(false);
    expect(canPersistModuleCollapse({ version: 1, entries: null })).toBe(false);
    // …and non-object inputs.
    expect(canPersistModuleCollapse("compact-v3")).toBe(false);
    expect(canPersistModuleCollapse(42)).toBe(false);
    expect(canPersistModuleCollapse([])).toBe(false);
  });

  it("stays in sync with setModuleCollapsed: available exactly when a patch is built", () => {
    // The gate and the patch builder are complementary by contract; a drift
    // between them would either disable a working toggle or enable one that
    // silently drops clicks. Probe the paired decision on every shape.
    const shapes: Array<{ name: string; doc: unknown }> = [
      { name: "v1", doc: { version: 1, entries: [{ id: "todo", order: 0, colSpan: 1 }] } },
      { name: "missing-undefined", doc: undefined },
      { name: "missing-null", doc: null },
      { name: "v2", doc: { version: 2, entries: [] } },
      { name: "no-version", doc: { entries: [] } },
      { name: "non-array-entries", doc: { version: 1, entries: "junk" } },
      { name: "string", doc: "compact-v3" },
      { name: "number", doc: 42 },
    ];
    for (const { doc } of shapes) {
      expect(canPersistModuleCollapse(doc)).toBe(setModuleCollapsed(doc, "todo", true) !== null);
    }
  });
});
