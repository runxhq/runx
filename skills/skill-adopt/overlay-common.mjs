export function record(value) {
  return isRecord(value) ? value : {};
}

export function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}

export function requiredString(value, field) {
  const parsed = stringValue(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

export function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

export function strings(value) {
  return Array.isArray(value) ? value.map(stringValue).filter(Boolean) : [];
}

export function uniqueStrings(value) {
  return [...new Set(strings(value))].sort();
}

export function records(value) {
  return Array.isArray(value) ? value.map(record) : [];
}

export function isRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value);
}

export function packageSegment(value, field) {
  const parsed = requiredString(value, field);
  if (!/^[a-z0-9][a-z0-9-]*$/u.test(parsed)) throw new Error(`${field} must be a lowercase package segment`);
  return parsed;
}

export function enumValue(value, allowed, field) {
  if (!allowed.includes(value)) throw new Error(`${field} must be one of ${allowed.join(", ")}`);
  return value;
}

export function numberValue(value) {
  return Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}

export function boundedMessage(error) {
  return (error instanceof Error ? error.message : "Binding validation failed")
    .replace(/\s+/gu, " ")
    .trim()
    .slice(0, 300);
}

const BOUNDS_FIELDS = ["allowed_output_prefix", "max_acts"];

export function normalizedBounds(value) {
  if (value === undefined || value === null) return { bounds: null, errors: [] };
  const errors = [];
  if (!isRecord(value)) return { bounds: null, errors: ["bounds must be an object"] };
  const unknown = Object.keys(value).filter((key) => !BOUNDS_FIELDS.includes(key));
  if (unknown.length > 0) errors.push(`bounds contains unknown fields: ${unknown.sort().join(", ")}`);
  const bounds = {};
  if (value.allowed_output_prefix !== undefined) {
    const prefix = normalizedPrefix(value.allowed_output_prefix, errors);
    if (prefix) bounds.allowed_output_prefix = prefix;
  }
  if (value.max_acts !== undefined) {
    if (!Number.isInteger(value.max_acts) || value.max_acts < 1 || value.max_acts > 10000) {
      errors.push("bounds.max_acts must be an integer between 1 and 10000");
    } else {
      bounds.max_acts = value.max_acts;
    }
  }
  if (errors.length === 0 && Object.keys(bounds).length === 0) {
    errors.push("bounds must declare allowed_output_prefix or max_acts");
  }
  return errors.length === 0 ? { bounds, errors } : { bounds: null, errors };
}

export function sameBounds(left, right) {
  const a = record(left);
  const b = record(right);
  if (Object.keys(b).some((key) => !BOUNDS_FIELDS.includes(key))) return false;
  return BOUNDS_FIELDS.every((field) => a[field] === b[field]);
}

function normalizedPrefix(value, errors) {
  const raw = stringValue(value);
  if (!raw || raw.length > 4000) {
    errors.push("bounds.allowed_output_prefix must be a non-empty path of at most 4000 characters");
    return null;
  }
  const absolute = raw.startsWith("/");
  const parts = raw.replaceAll("\\", "/").split("/");
  if (parts.some((part) => part === "..")) {
    errors.push("bounds.allowed_output_prefix must not contain parent traversal");
    return null;
  }
  const normalized = parts.filter((part) => part && part !== ".").join("/");
  if (!normalized) {
    errors.push("bounds.allowed_output_prefix must name a directory prefix");
    return null;
  }
  return absolute ? `/${normalized}` : normalized;
}
