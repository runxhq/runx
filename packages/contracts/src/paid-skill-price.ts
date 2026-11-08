import { canonicalJsonStringify } from "./canonical-json.js";
import type {
  PaidSkillPriceCaseContract,
  PaidSkillPriceContract,
  RequiredLeaseContract,
} from "./generated/contract-types.js";

/** A value a price case matches: a string, a portable integer or a boolean. */
type PaidSkillPriceChoiceContract = PaidSkillPriceCaseContract["inputs"][string];

/**
 * Paid skill prices, as the Rust contract computes them:
 * `fixtures/contracts/paid-skill-price` holds the vectors both agree on.
 */

const MAX_PORTABLE_INTEGER = BigInt(Number.MAX_SAFE_INTEGER);
const MAX_PRICED_INPUTS = 4;
const MAX_PRICE_CASES = 256;

export type PaidSkillPriceErrorCode =
  | "invalid"
  | "missing_input"
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

/** The inputs a price reads, in name order. */
function pricedInputs(price: PaidSkillPriceContract): readonly string[] {
  switch (price.basis) {
    case "fixed":
      return [];
    case "by_input":
      return Object.keys(price.cases[0]?.inputs ?? {}).sort();
    case "by_size":
      return [price.input];
  }
}

/**
 * Why a price breaks its own invariants, or undefined: every case names the
 * same one to four priced inputs, each combination of their values has one
 * case, and a size price's largest amount is still a portable integer.
 */
export function paidSkillPriceProblem(price: PaidSkillPriceContract): string | undefined {
  switch (price.basis) {
    case "fixed":
      return undefined;
    case "by_input": {
      const { cases } = price;
      if (cases.length === 0 || cases.length > MAX_PRICE_CASES) {
        return "a by_input price has one to 256 cases";
      }
      const priced = pricedInputs(price);
      if (priced.length === 0 || priced.length > MAX_PRICED_INPUTS) {
        return "a by_input price reads one to four inputs";
      }
      const seen = new Set<string>();
      for (const priceCase of cases) {
        const names = Object.keys(priceCase.inputs).sort();
        if (names.length !== priced.length || names.some((name, index) => name !== priced[index])) {
          return "each price case names the same priced inputs";
        }
        const key = canonicalJsonStringify(priceCase.inputs);
        if (seen.has(key)) return "each combination of priced input values has one case";
        seen.add(key);
      }
      return undefined;
    }
    case "by_size":
      return artifactAmount(price, BigInt(price.max_bytes)) === undefined
        ? "a by_size price must stay a portable amount up to max_bytes"
        : undefined;
  }
}

const STORED_ARTIFACT_REF = /^runx:artifact:sha256:[0-9a-f]{64}$/u;

/**
 * The stored artifact a `by_size` price reads: the `artifact_uri` its input
 * holds. Its committed size, looked up by the caller, is what the call costs.
 */
export function paidSkillPricedArtifactRef(
  price: PaidSkillPriceContract,
  inputs: Readonly<Record<string, unknown>>,
): string | undefined {
  if (price.basis !== "by_size" || !Object.hasOwn(inputs, price.input)) return undefined;
  const value = inputs[price.input];
  if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
  const reference = (value as Record<string, unknown>).artifact_uri;
  return typeof reference === "string" && STORED_ARTIFACT_REF.test(reference) ? reference : undefined;
}

/**
 * What one call with `inputs` costs. A `by_size` price reads the committed
 * size of the artifact its input names, looked up before payment.
 */
export function paidSkillPriceAmount(
  price: PaidSkillPriceContract,
  inputs: Readonly<Record<string, unknown>>,
  artifactSizeBytes?: number,
): number {
  switch (price.basis) {
    case "fixed":
      return price.amount_minor;
    case "by_input": {
      for (const name of pricedInputs(price)) {
        if (!Object.hasOwn(inputs, name)) {
          throw new PaidSkillPriceError("missing_input", `priced input ${name} is missing`);
        }
      }
      const matched = price.cases.find((priceCase) =>
        Object.entries(priceCase.inputs).every(([name, choice]) => sameChoice(choice, inputs[name]))
      );
      if (!matched) throw new PaidSkillPriceError("no_case", "no price case matches these inputs");
      return matched.amount_minor;
    }
    case "by_size": {
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
  price: Extract<PaidSkillPriceContract, { basis: "by_size" }>,
  sizeBytes: bigint,
): number | undefined {
  const unitBytes = BigInt(price.unit_bytes);
  const units = (sizeBytes + unitBytes - 1n) / unitBytes;
  const amount = units * BigInt(price.unit_amount_minor) + BigInt(price.base_amount_minor);
  return amount >= 1n && amount <= MAX_PORTABLE_INTEGER ? Number(amount) : undefined;
}

/**
 * Why the lease a paid method's buyer must hold breaks its contract, or
 * undefined: its target binds the method's inputs, and nothing else, so it
 * names the leased resource before the purchase.
 */
export function paidSkillRequiredLeaseProblem(lease: RequiredLeaseContract): string | undefined {
  const bindings = new Set<string>();
  collectBindings(lease.target, bindings);
  if (!bindings.has("$argument") || [...bindings].some((binding) => binding !== "$argument")) {
    return "requires_lease target is rendered from the method's inputs alone";
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
