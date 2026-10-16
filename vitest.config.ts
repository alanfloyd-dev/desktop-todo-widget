import { defineConfig } from "vitest/config";

/**
 * The frontend unit-test runner (composable state transitions only; the
 * component surface is exercised by the manual QA pass and the browser
 * preview). Node environment: the controllers under test are plain reactive
 * state machines with an injected bridge, no DOM required.
 */
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
