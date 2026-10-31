import { describe, expect, it } from "vitest";
import { en, zhHans } from "./catalog";

/**
 * The catalogs' structural contract: both locales carry exactly the same key
 * set, so a localized surface can never render an untranslated raw key in
 * one language while working in the other. The G2 GitHub keys ride the same
 * invariant as every other module's copy.
 */
describe("i18n catalog parity", () => {
  it("both locales carry exactly the same key set", () => {
    const enKeys = Object.keys(en).sort();
    const zhKeys = Object.keys(zhHans).sort();
    expect(zhKeys).toEqual(enKeys);
  });

  it("carries the GitHub search plane's copy in both locales (G2)", () => {
    for (const key of [
      "github.openIssues",
      "github.pullRequests",
      "github.searchLoading",
      "github.searchUnavailable",
      "github.error.searchIncomplete",
    ]) {
      expect(en[key], `en missing ${key}`).toBeTruthy();
      expect(zhHans[key], `zh-Hans missing ${key}`).toBeTruthy();
    }
  });

  it("does not leak interpolation placeholders between locales", () => {
    const placeholder = /\{(\w+)\}/g;
    for (const key of Object.keys(en)) {
      const enParams = [...en[key].matchAll(placeholder)].map((m) => m[1]).sort();
      const zhParams = [...(zhHans[key] ?? "").matchAll(placeholder)].map((m) => m[1]).sort();
      expect(zhParams, `placeholder mismatch on ${key}`).toEqual(enParams);
    }
  });
});
