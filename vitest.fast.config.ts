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
      "tests/client-native.test.ts",
      "tests/least-privilege-scope.test.ts",
      "tests/scope-policy-differential.test.ts",
      "tests/eval-binaries-read-one-document.test.ts",
      "tests/runtime-architecture.test.ts",
      "tests/core-skill-inventory.test.ts",
      "tests/policy-draft-validation.test.ts",
      "tests/runx-cli-release-evidence.test.ts",
      "tests/stripe-spt-rail-adapter.test.ts",
      "tests/independent-verifier.test.ts",
    ],
    // A test ends on its own events, a process's exit or an answer, never on
    // a clock.
    testTimeout: 0,
    hookTimeout: 0,
  },
});
