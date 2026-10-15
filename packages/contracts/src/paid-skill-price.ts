import { canonicalJsonStringify } from "./canonical-json.js";
import type {
  PaidSkillActsOnContract,
  PaidSkillPriceChoiceContract,
  PaidSkillPriceContract,
} from "./schemas/paid-invocation.js";

/**
 * Paid skill prices, as the Rust contract computes them:
 * `fixtures/contracts/paid-skill-price` holds the vectors both agree on.
 */

const MAX_PORTABLE_INTEGER = BigInt(Number.MAX_SAFE_INTEGER);
const MAX_PRICE_SELECTORS = 4;
const MAX_PRICE_CASES = 256;

export type PaidSkillPriceErrorCode =
  | "invalid"
  | "missing_selector"
  | "no_case"
  | "artifact_size_unknown"
  | "artifact_empty"
  | "artifact_too_large";

export class PaidSkillPriceError extends Error {
  constructor(readonly code: PaidSkillPriceErrorCode, message: string) {
    super(message);
    this.name = "PaidSkillPriceError";
  }
}

/**
 * Why a price breaks its own invariants, or undefined: unique selectors, one
 * case per combination with one value per selector, and a byte price whose
 * largest amount is still a portable integer.
 */
export function paidSkillPriceProblem(price: PaidSkillPriceContract): string | undefined {
  switch (price.model) {
    case "fixed":
      return undefined;
    case "input_choice": {
      const { selectors, cases } = price;
      if (selectors.length === 0 || selectors.length > MAX_PRICE_SELECTORS) {
        return "an input_choice price names one to four selectors";
      }
      if (new Set(selectors).size !== selectors.length) {
        return "an input_choice price names each selector once";
      }
      if (cases.length === 0 || cases.length > MAX_PRICE_CASES) {
        return "an input_choice price has one to 256 cases";
      }
      const seen = new Set<string>();
      for (const priceCase of cases) {
        if (priceCase.when.length !== selectors.length) {
          return "each price case gives one value per selector";
        }
        const key = canonicalJsonStringify(priceCase.when);
        if (seen.has(key)) return "each combination of selector values has one price case";
        seen.add(key);
      }
      return undefined;
    }
    case "artifact_bytes":
      return artifactAmount(price, BigInt(price.max_bytes)) === undefined
        ? "an artifact_bytes price must stay a portable amount up to max_bytes"
        : undefined;
  }
}

/** The input names a price reads. */
export function paidSkillPricedInputs(price: PaidSkillPriceContract): readonly string[] {
  switch (price.model) {
    case "fixed":
      return [];
    case "input_choice":
      return price.selectors;
    case "artifact_bytes":
      return [price.artifact_input];
  }
}

const STORED_ARTIFACT_REF = /^runx:artifact:sha256:[0-9a-f]{64}$/u;

/**
 * The stored artifact an `artifact_bytes` price reads: the `artifact_ref` its
 * input holds. Its committed size, looked up by the caller, is what the call
 * costs.
 */
export function paidSkillPricedArtifactRef(
  price: PaidSkillPriceContract,
  inputs: Readonly<Record<string, unknown>>,
): string | undefined {
  if (price.model !== "artifact_bytes" || !Object.hasOwn(inputs, price.artifact_input)) return undefined;
  const value = inputs[price.artifact_input];
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const reference = (value as Record<string, unknown>).artifact_ref;
  return typeof reference === "string" && STORED_ARTIFACT_REF.test(reference) ? reference : undefined;
}

/**
 * What one call with `inputs` costs. An `artifact_bytes` price reads the
 * committed size of the artifact its input names, looked up before payment.
 */
export function paidSkillPriceAmount(
  price: PaidSkillPriceContract,
  inputs: Readonly<Record<string, unknown>>,
  artifactSizeBytes?: number,
): number {
  switch (price.model) {
    case "fixed":
      return price.amount_minor;
    case "input_choice": {
      const values = price.selectors.map((selector) => {
        if (!Object.hasOwn(inputs, selector)) {
          throw new PaidSkillPriceError("missing_selector", `price selector input ${selector} is missing`);
        }
        return inputs[selector];
      });
      const matched = price.cases.find((priceCase) =>
        priceCase.when.every((choice, index) => sameChoice(choice, values[index]))
      );
      if (!matched) throw new PaidSkillPriceError("no_case", "no price case matches these inputs");
      return matched.amount_minor;
    }
    case "artifact_bytes": {
      if (artifactSizeBytes === undefined) {
        throw new PaidSkillPriceError("artifact_size_unknown", "the priced artifact's committed size is unknown");
      }
      if (!Number.isSafeInteger(artifactSizeBytes) || artifactSizeBytes < 0) {
        throw new PaidSkillPriceError("invalid", "the priced artifact's size is not a byte count");
      }
      if (artifactSizeBytes === 0) {
        throw new PaidSkillPriceError("artifact_empty", "the priced artifact is empty");
      }
      if (artifactSizeBytes > price.max_bytes) {
        throw new PaidSkillPriceError(
          "artifact_too_large",
          `the priced artifact is ${artifactSizeBytes} bytes, over the offer's ${price.max_bytes}`,
        );
      }
      const amount = artifactAmount(price, BigInt(artifactSizeBytes));
      if (amount === undefined) throw new PaidSkillPriceError("invalid", "the artifact price overflows");
      return amount;
    }
  }
}

function sameChoice(choice: PaidSkillPriceChoiceContract, value: unknown): boolean {
  return typeof choice === typeof value && choice === value;
}

function artifactAmount(
  price: Extract<PaidSkillPriceContract, { model: "artifact_bytes" }>,
  sizeBytes: bigint,
): number | undefined {
  const unitBytes = BigInt(price.unit_bytes);
  const units = (sizeBytes + unitBytes - 1n) / unitBytes;
  const amount = units * BigInt(price.unit_amount_minor) + BigInt(price.base_amount_minor);
  return amount >= 1n && amount <= MAX_PORTABLE_INTEGER ? Number(amount) : undefined;
}

/**
 * Why what a paid runner acts on breaks its contract, or undefined: its
 * target binds the runner's inputs, and nothing else, so it names the held
 * resource before the purchase.
 */
export function paidSkillActsOnProblem(actsOn: PaidSkillActsOnContract): string | undefined {
  const bindings = new Set<string>();
  collectBindings(actsOn.target, bindings);
  if (!bindings.has("$argument") || [...bindings].some((binding) => binding !== "$argument")) {
    return "acts_on target is rendered from the runner's inputs alone";
  }
  return undefined;
}

function collectBindings(value: unknown, bindings: Set<string>): void {
  if (Array.isArray(value)) {
    for (const item of value) collectBindings(item, bindings);
    return;
  }
  if (typeof value !== "object" || value === null) return;
  const entries = Object.entries(value);
  const [first] = entries;
  if (entries.length === 1 && first && first[0].startsWith("$")) {
    const [key, argument] = first;
    if (key === "$concat") {
      collectBindings(argument, bindings);
    } else {
      bindings.add(key);
    }
    return;
  }
  for (const [, nested] of entries) collectBindings(nested, bindings);
}
