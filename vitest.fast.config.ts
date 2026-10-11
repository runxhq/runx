import { defineConfig } from "vitest/config";

import { workspaceAliases } from "./vitest.workspace-aliases.js";

export default defineConfig({
  resolve: {
    alias: [...workspaceAliases],
  },
  test: {
    include: [
      "packages/**/*.test.ts",
      "tests/openai-adapter.test.ts",
      "tests/anthropic-adapter.test.ts",
      "tests/vercel-ai-adapter.test.ts",
      "tests/langchain-adapter.test.ts",
      "tests/crewai-adapter.test.ts",
      "tests/kernel-parity-fixtures.test.ts",
      "tests/cli-feature-parity.test.ts",
      "tests/least-privilege-scope.test.ts",
      "tests/scope-policy-differential.test.ts",
      "tests/runtime-architecture.test.ts",
      "tests/policy-author-validation.test.ts",
      "tests/runx-cli-release-evidence.test.ts",
      "tests/stripe-spt-rail-adapter.test.ts",
      "tests/independent-verifier.test.ts",
    ],
    // These suites shell out to the debug `runx` binary; the generous timeouts
    // absorb its cold start under parallel load.
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});
