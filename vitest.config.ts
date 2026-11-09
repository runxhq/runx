import { defineConfig } from "vitest/config";

import { workspaceAliases } from "./vitest.workspace-aliases.js";

export default defineConfig({
  resolve: {
    alias: [...workspaceAliases],
  },
  test: {
    // Native subprocess suites share the host with their workers.
    maxWorkers: 4,
    // A test ends on its own events, a process's exit or an answer, never on
    // a clock.
    testTimeout: 0,
    hookTimeout: 0,
    include: ["packages/**/*.test.ts", "tests/*.test.ts", "tests/examples/**/*.test.ts"],
  },
});
