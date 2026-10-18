import { describe, expect, it } from "vitest";
import type { QuickLink } from "../../types";
import { openQuickLink, type ShortcutsBridge } from "./bridge";
import { moveLink, newLinkId, quickLinkUrlErrorKey } from "./links";

function link(id: string): QuickLink {
  return { id, name: `Link ${id}`, url: `https://example.com/${id}` };
}

describe("new link identity", () => {
  it("issues non-empty ids that differ across calls", () => {
    const first = newLinkId();
    const second = newLinkId();
    expect(first.length).toBeGreaterThan(0);
    expect(second.length).toBeGreaterThan(0);
    expect(first).not.toBe(second);
  });
});

describe("client-side URL check", () => {
  it("an empty or whitespace-only URL asks for a URL", () => {
    expect(quickLinkUrlErrorKey("")).toBe("quickLinks.error.urlRequired");
    expect(quickLinkUrlErrorKey("   ")).toBe("quickLinks.error.urlRequired");
  });

  it("an unparsable URL is invalid", () => {
    expect(quickLinkUrlErrorKey("not a url")).toBe("quickLinks.error.invalidUrl");
  });

  it("only http and https are accepted", () => {
    expect(quickLinkUrlErrorKey("ftp://example.com/file")).toBe("quickLinks.error.invalidUrl");
    expect(quickLinkUrlErrorKey("javascript:alert(1)")).toBe("quickLinks.error.invalidUrl");
  });

  it("a well-formed http(s) URL passes", () => {
    expect(quickLinkUrlErrorKey("http://example.com/")).toBeNull();
    expect(quickLinkUrlErrorKey("https://example.com/docs?page=1")).toBeNull();
  });
});

describe("moving a link", () => {
  it("moving down swaps the link with its successor in place", () => {
    const links = [link("a"), link("b"), link("c")];
    moveLink(links, "a", 1);
    expect(links.map((item) => item.id)).toEqual(["b", "a", "c"]);
  });

  it("moving up swaps the link with its predecessor in place", () => {
    const links = [link("a"), link("b"), link("c")];
    moveLink(links, "c", -1);
    expect(links.map((item) => item.id)).toEqual(["a", "c", "b"]);
  });

  it("the first row cannot move up and the last row cannot move down", () => {
    const links = [link("a"), link("b")];
    moveLink(links, "a", -1);
    expect(links.map((item) => item.id)).toEqual(["a", "b"]);
    moveLink(links, "b", 1);
    expect(links.map((item) => item.id)).toEqual(["a", "b"]);
  });

  it("an unknown id is a no-op", () => {
    const links = [link("a"), link("b")];
    moveLink(links, "missing", 1);
    expect(links.map((item) => item.id)).toEqual(["a", "b"]);
  });
});

describe("shortcuts bridge", () => {
  /**
   * The bridge's `invoke` is runtime-generic (any result type); a test double
   * with concrete return types needs the cast to fit that signature. The same
   * idiom as the weather controller tests.
   */
  function fakeBridge() {
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    const bridge: ShortcutsBridge = {
      invoke: ((command: string, args?: Record<string, unknown>) => {
        calls.push({ command, args: args ?? {} });
        return Promise.resolve();
      }) as unknown as ShortcutsBridge["invoke"],
    };
    return { bridge, calls };
  }

  it("opens a link by its stable id through the pinned command name", async () => {
    const { bridge, calls } = fakeBridge();
    await openQuickLink(bridge, "link-1");
    expect(calls).toEqual([{ command: "open_quick_link", args: { id: "link-1" } }]);
  });
});
