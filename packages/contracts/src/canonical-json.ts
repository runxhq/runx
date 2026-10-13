import { createHash } from "node:crypto";

export const RUNX_STABLE_JSON_V1 = "runx.stable-json.v1" as const;

export function canonicalJsonStringify(value: unknown): string {
  return canonicalJsonValue(value, "$", new Set<object>());
}

export function sha256Hex(value: string | Uint8Array): string {
  return createHash("sha256").update(value).digest("hex");
}

export function sha256Prefixed(value: string | Uint8Array): `sha256:${string}` {
  return `sha256:${sha256Hex(value)}`;
}

function canonicalJsonValue(value: unknown, path: string, stack: Set<object>): string {
  if (value === null) {
    return "null";
  }

  switch (typeof value) {
    case "boolean":
      return value ? "true" : "false";
    case "number":
      return canonicalJsonNumber(value, path);
    case "string":
      return canonicalJsonString(value, path);
    case "undefined":
      throw unsupported(path, "undefined");
    case "function":
      throw unsupported(path, "function");
    case "symbol":
      throw unsupported(path, "symbol");
    case "bigint":
      return value.toString();
    case "object":
      return Array.isArray(value)
        ? canonicalJsonArray(value, path, stack)
        : canonicalJsonObject(value, path, stack);
  }
  throw unsupported(path, "value");
}

function canonicalJsonNumber(value: number, path: string): string {
  if (Number.isNaN(value)) {
    throw unsupported(path, "NaN");
  }
  if (value === Infinity) {
    throw unsupported(path, "Infinity");
  }
  if (value === -Infinity) {
    throw unsupported(path, "-Infinity");
  }

  const serialized = JSON.stringify(value);
  if (typeof serialized !== "string") {
    throw unsupported(path, "number");
  }
  return serialized;
}

function canonicalJsonString(value: string, path: string): string {
  assertNoUnpairedSurrogate(value, path);
  const serialized = JSON.stringify(value);
  if (typeof serialized !== "string") {
    throw new Error(`${RUNX_STABLE_JSON_V1}: failed to serialize string`);
  }
  return serialized;
}

function canonicalJsonArray(value: readonly unknown[], path: string, stack: Set<object>): string {
  assertAcyclic(value, path, stack);
  assertNoEnumerableSymbolKeys(value, path);
  try {
    const parts: string[] = [];
    for (let index = 0; index < value.length; index += 1) {
      if (!Object.prototype.hasOwnProperty.call(value, index)) {
        throw unsupported(indexPath(path, index), "array hole");
      }
      parts.push(canonicalJsonValue(value[index], indexPath(path, index), stack));
    }

    const extraKey = Object.keys(value).find((key) => !isArrayElementKey(key, value.length));
    if (extraKey !== undefined) {
      throw unsupported(propertyPath(path, extraKey), "array property");
    }

    return `[${parts.join(",")}]`;
  } finally {
    stack.delete(value);
  }
}

function canonicalJsonObject(value: object, path: string, stack: Set<object>): string {
  if (!isPlainJsonObject(value)) {
    throw unsupported(path, "non-plain object");
  }

  assertAcyclic(value, path, stack);
  assertNoEnumerableSymbolKeys(value, path);
  try {
    const record = value as Record<string, unknown>;
    const entries = Object.keys(record)
      .sort(compareJsonObjectKeys)
      .map((key) => {
        const keyPath = propertyPath(path, key);
        return `${canonicalJsonString(key, keyPath)}:${canonicalJsonValue(record[key], keyPath, stack)}`;
      });
    return `{${entries.join(",")}}`;
  } finally {
    stack.delete(value);
  }
}

function compareJsonObjectKeys(left: string, right: string): number {
  const leftIterator = left[Symbol.iterator]();
  const rightIterator = right[Symbol.iterator]();

  while (true) {
    const leftNext = leftIterator.next();
    const rightNext = rightIterator.next();
    if (leftNext.done && rightNext.done) {
      return 0;
    }
    if (leftNext.done) {
      return -1;
    }
    if (rightNext.done) {
      return 1;
    }

    const diff = leftNext.value.codePointAt(0)! - rightNext.value.codePointAt(0)!;
    if (diff !== 0) {
      return diff;
    }
  }
}

function assertAcyclic(value: object, path: string, stack: Set<object>): void {
  if (stack.has(value)) {
    throw unsupported(path, "cyclic object");
  }
  stack.add(value);
}

function assertNoEnumerableSymbolKeys(value: object, path: string): void {
  const hasEnumerableSymbolKey = Object.getOwnPropertySymbols(value)
    .some((symbol) => Object.prototype.propertyIsEnumerable.call(value, symbol));
  if (hasEnumerableSymbolKey) {
    throw unsupported(path, "symbol key");
  }
}

function assertNoUnpairedSurrogate(value: string, path: string): void {
  for (let index = 0; index < value.length; index += 1) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) {
        throw unsupported(indexPath(path, index), "unpaired surrogate");
      }
      index += 1;
      continue;
    }
    if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw unsupported(indexPath(path, index), "unpaired surrogate");
    }
  }
}

function isPlainJsonObject(value: object): boolean {
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function isArrayElementKey(key: string, length: number): boolean {
  if (key === "") {
    return false;
  }
  const index = Number(key);
  return Number.isInteger(index) && index >= 0 && index < length && String(index) === key;
}

function propertyPath(path: string, key: string): string {
  return `${path}[${JSON.stringify(key)}]`;
}

function indexPath(path: string, index: number): string {
  return `${path}[${index}]`;
}

function unsupported(path: string, kind: string): Error {
  return new Error(`${RUNX_STABLE_JSON_V1}: unsupported ${kind} at ${path}`);
}
