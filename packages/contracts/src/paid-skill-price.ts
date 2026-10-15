import { canonicalJsonStringify } from "./canonical-json.js";
import type {
  PaidSkillClaimContract,
  PaidSkillPriceChoiceContract,
  PaidSkillPriceContract,
} from "./schemas/paid-invocation.js";

/**
 * Paid skill prices and claim endpoints, as the Rust contract computes them:
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

const MAX_CLAIM_URL_LENGTH = 512;
const MAX_CLAIM_POINTER_SEGMENTS = 8;
const MAX_CLAIM_VALUE_LENGTH = 128;
const PATH_TEXT = /^[A-Za-z0-9._~\/-]*$/u;
const PARAMETER_NAME = /^[a-z][a-z0-9_]{0,31}$/u;
const HOST_LABEL = /^[a-z0-9-]{1,63}$/u;
const PLAIN_SEGMENT = /^[A-Za-z0-9._~-]{1,128}$/u;

export type PaidSkillClaimErrorCode = "invalid" | "unresolved";

export class PaidSkillClaimError extends Error {
  constructor(readonly code: PaidSkillClaimErrorCode, message: string) {
    super(message);
    this.name = "PaidSkillClaimError";
  }
}

/** Why a claim endpoint is not https on a fixed host with bound path parameters, or undefined. */
export function paidSkillClaimProblem(claim: PaidSkillClaimContract): string | undefined {
  const { url } = claim;
  const bind = claim.bind ?? {};
  if (url.length > MAX_CLAIM_URL_LENGTH) return "claim url is too long";
  if (!url.startsWith("https://")) return "claim url must be https";
  const rest = url.slice("https://".length);
  const slash = rest.indexOf("/");
  if (slash < 0) return "claim url must have a path";
  if (!validAuthority(rest.slice(0, slash))) return "claim url must name a fixed host";
  const parameters = pathParameters(rest.slice(slash + 1));
  if (!parameters) return "claim url path is invalid";
  const bound = Object.keys(bind);
  if (bound.length !== parameters.size || bound.some((name) => !parameters.has(name))) {
    return "every claim url parameter is bound once, and only those";
  }
  const pointer = Object.values(bind).find((value) => !validPointer(value));
  if (pointer !== undefined) {
    return `claim binding ${pointer} is not a JSON pointer of one to eight segments`;
  }
  return undefined;
}

/**
 * The claim endpoint for a fulfilled purchase whose public result is
 * `result`. A bound value must be a plain path segment, so the result cannot
 * redirect the claim.
 */
export function resolvePaidSkillClaim(claim: PaidSkillClaimContract, result: unknown): string {
  let url = claim.url;
  for (const [parameter, pointer] of Object.entries(claim.bind ?? {})) {
    const value = jsonPointer(result, pointer);
    if (typeof value !== "string" || !plainSegment(value)) {
      throw new PaidSkillClaimError(
        "unresolved",
        `claim parameter ${parameter} is not a plain path segment in the result`,
      );
    }
    url = url.replaceAll(`{${parameter}}`, value);
  }
  return url;
}

function validAuthority(authority: string): boolean {
  const colon = authority.indexOf(":");
  const host = colon < 0 ? authority : authority.slice(0, colon);
  const port = colon < 0 ? undefined : authority.slice(colon + 1);
  const labels = host.split(".");
  const hostValid = labels.length >= 2
    && labels.every((label) => HOST_LABEL.test(label) && !label.startsWith("-") && !label.endsWith("-"));
  const portValid = port === undefined
    || (/^[0-9]{1,5}$/u.test(port) && Number(port) > 0 && Number(port) <= 65_535);
  return hostValid && portValid;
}

function pathParameters(path: string): Set<string> | undefined {
  const parameters = new Set<string>();
  let rest = path;
  for (;;) {
    const open = rest.indexOf("{");
    if (open < 0) return PATH_TEXT.test(rest) ? parameters : undefined;
    if (!PATH_TEXT.test(rest.slice(0, open))) return undefined;
    const close = rest.indexOf("}", open);
    if (close < 0) return undefined;
    const name = rest.slice(open + 1, close);
    if (!PARAMETER_NAME.test(name) || parameters.has(name)) return undefined;
    parameters.add(name);
    rest = rest.slice(close + 1);
  }
}

function validPointer(pointer: string): boolean {
  if (!pointer.startsWith("/")) return false;
  const segments = pointer.slice(1).split("/");
  return segments.length <= MAX_CLAIM_POINTER_SEGMENTS
    && segments.every((segment) => segment.length > 0 && !/~(?![01])/u.test(segment));
}

function jsonPointer(value: unknown, pointer: string): unknown {
  if (!pointer.startsWith("/")) return undefined;
  let current: unknown = value;
  for (const segment of pointer.slice(1).split("/")) {
    const key = segment.replaceAll("~1", "/").replaceAll("~0", "~");
    if (Array.isArray(current)) {
      if (!/^(0|[1-9][0-9]*)$/u.test(key)) return undefined;
      current = current[Number(key)];
    } else if (typeof current === "object" && current !== null) {
      if (!Object.hasOwn(current, key)) return undefined;
      current = (current as Record<string, unknown>)[key];
    } else {
      return undefined;
    }
  }
  return current;
}

function plainSegment(value: string): boolean {
  return value.length <= MAX_CLAIM_VALUE_LENGTH
    && PLAIN_SEGMENT.test(value)
    && value !== "."
    && value !== "..";
}
