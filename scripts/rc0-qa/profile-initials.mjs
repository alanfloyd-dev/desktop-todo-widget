// Verify the profile-initials derivation by running the real frontend function.
//
// `profileInitials` is TypeScript, so `cargo test` can only pin its shape (see
// `profile_initials_have_no_builtin_fallback_identity` in settings.rs). This runs
// the actual implementation, including the Unicode path, which is the part that
// matters for CJK display names.
//
//   node --experimental-strip-types scripts/rc0-qa/profile-initials.mjs
//
// QA tooling only; nothing in the product imports it.

const { profileInitials } = await import("../../src/appearance.ts");

const cases = [
  // The shipped neutral default must be a single initial, not `US`.
  ["User", "U"],
  ["用户", "用"],
  // A user-configured name keeps the familiar two-letter form.
  ["Alan Floyd", "AF"],
  ["alan floyd", "AF"],
  ["Mary Jane Watson", "MW"],
  // Single letters and CJK names.
  ["a", "A"],
  ["Ω", "Ω"],
  ["李 明", "李明"],
  // No name, no initials: never a built-in mark.
  ["", ""],
  ["   ", ""],
  ["\t\n", ""],
];

let failures = 0;
for (const [input, expected] of cases) {
  const actual = profileInitials(input);
  const ok = actual === expected;
  if (!ok) failures += 1;
  console.log(
    `${ok ? "PASS" : "FAIL"}  profileInitials(${JSON.stringify(input)}) = ${JSON.stringify(actual)}` +
      (ok ? "" : `  expected ${JSON.stringify(expected)}`),
  );
}

// The derivation must agree with the persisted default, so a fresh install shows
// exactly one neutral initial.
const shippedDefault = "User";
console.log(
  `\nshipped default ${JSON.stringify(shippedDefault)} -> ${JSON.stringify(profileInitials(shippedDefault))}`,
);
if (profileInitials(shippedDefault) !== "U") {
  console.error("the shipped default does not render as a single `U`");
  failures += 1;
}

if (failures > 0) {
  console.error(`\n${failures} case(s) failed`);
  process.exitCode = 1;
} else {
  console.log("\nall derivation cases pass");
}
