import { defineConfig } from "vitest/config";

import { workspaceAliases } from "./vitest.workspace-aliases.js";

export default defineConfig({
  resolve: {
    alias: [...workspaceAliases],
  },
  test: {
    include: ["tests/product/**/*.test.ts"],
    // A product journey ends on its own events, a process's exit or a
    // registry's answer, never on a clock.
    testTimeout: 0,
    hookTimeout: 0,
  },
});
