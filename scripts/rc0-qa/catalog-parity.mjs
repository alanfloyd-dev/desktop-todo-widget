// Local QA helper: verifies that the English and Simplified Chinese catalogs
// declare exactly the same keys, and reports the Quick Links key set.
// Not part of the build; run with `node scripts/rc0-qa/catalog-parity.mjs`.
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("../../src/i18n/catalog.ts", import.meta.url), "utf8");
const zhStart = source.indexOf("export const zhHans");
const enBlock = source.slice(0, zhStart);
const zhBlock = source.slice(zhStart);

const keys = (block) => [...block.matchAll(/^\s*"([^"]+)":/gm)].map((match) => match[1]);
const en = new Set(keys(enBlock));
const zh = new Set(keys(zhBlock));

const missingInZh = [...en].filter((key) => !zh.has(key));
const missingInEn = [...zh].filter((key) => !en.has(key));

console.log(`en keys: ${en.size}`);
console.log(`zh-Hans keys: ${zh.size}`);
console.log(`missing in zh-Hans: ${missingInZh.length ? missingInZh.join(", ") : "none"}`);
console.log(`missing in en: ${missingInEn.length ? missingInEn.join(", ") : "none"}`);
console.log(`quickLinks keys: en=${[...en].filter((k) => k.startsWith("quickLinks")).length} zh=${[...zh].filter((k) => k.startsWith("quickLinks")).length}`);

if (missingInZh.length || missingInEn.length) {
  process.exitCode = 1;
}
