import { defineConfig } from "vitest/config";

import { workspaceAliases } from "./vitest.workspace-aliases.js";

export default defineConfig({
  resolve: {
    alias: [...workspaceAliases],
  },
  test: {
    include: ["packages/**/*.test.ts", "tests/*.test.ts", "tests/examples/**/*.test.ts"],
  },
});
