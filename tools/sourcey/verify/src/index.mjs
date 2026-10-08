import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { defineTool, failure, isRecord } from "@runxhq/extension-sdk";

export default defineTool({
  name: "sourcey.verify",
  run({ inputs, env }) {
    const inputBase = env.RUNX_CWD || process.cwd();
    const outputDir = path.resolve(inputBase, inputs.output_dir);
    const indexPath = path.resolve(outputDir, inputs.index_path || "index.html");
    if (!existsSync(indexPath)) {
      throw new Error(`Sourcey output is missing index.html at ${indexPath}`);
    }

    const contents = readFileSync(indexPath, "utf8");
    const iconValidation = isRecord(inputs.sourcey_build_report?.icon_validation)
      ? inputs.sourcey_build_report.icon_validation
      : undefined;
    if (iconValidation?.status === "invalid") {
      return failure(
        {
          output_dir: outputDir,
          index_path: indexPath,
          verified: false,
          contains_doctype: /<!doctype html>/i.test(contents),
          error: "invalid_sourcey_card_icons",
          icon_validation: iconValidation,
        },
        {
          stderr: `Sourcey card icon validation failed: ${iconValidation.invalid_count ?? "unknown"} invalid Heroicon name(s).`,
        },
      );
    }

    return {
      output_dir: outputDir,
      index_path: indexPath,
      verified: true,
      contains_doctype: /<!doctype html>/i.test(contents),
      icon_validation: iconValidation,
    };
  },
});
