import { describe, expect, it } from "vitest";
import { MODULE_REGISTRY, type ModuleId } from "./registry";
import {
  DASHBOARD_MODULE_IDS,
  DEFAULT_MODULE_LAYOUT,
  hasColumnSpan,
  resolveDashboardLayout,
  resolveModuleLayout,
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
