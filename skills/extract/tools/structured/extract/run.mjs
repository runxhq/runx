import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const TOOL_VERSION = "0.1.0";

function readInputs() {
  const raw = process.env.RUNX_INPUTS_PATH
    ? fs.readFileSync(process.env.RUNX_INPUTS_PATH, "utf8")
    : (process.env.RUNX_INPUTS_JSON || "{}");
  return JSON.parse(raw);
}

function packageRoot() {
  return path.resolve(__dirname, "../../..");
}

function resolveInsidePackage(relativePath, label) {
  const root = packageRoot();
  const resolved = path.resolve(root, String(relativePath || ""));
  if (!resolved.startsWith(root + path.sep) && resolved !== root) {
    throw new Error(`${label} escapes the skill package`);
  }
  return resolved;
}

function sha256Bytes(bytes) {
  return `sha256:${crypto.createHash("sha256").update(bytes).digest("hex")}`;
}

function sha256Text(text) {
  return sha256Bytes(Buffer.from(text, "utf8"));
}

function canonicalJson(value) {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function schemaTypeOf(value) {
  if (value === null) {
    return "null";
  }
  if (Array.isArray(value)) {
    return "array";
  }
  if (Number.isInteger(value)) {
    return "integer";
  }
  return typeof value;
}

function resolveSchemaRef(ref, rootSchema) {
  if (!ref.startsWith("#/")) {
    throw new Error(`unsupported schema ref: ${ref}`);
  }
  return ref
    .slice(2)
    .split("/")
    .reduce((node, part) => {
      const key = part.replace(/~1/g, "/").replace(/~0/g, "~");
      if (!node || typeof node !== "object" || !(key in node)) {
        throw new Error(`unresolved schema ref: ${ref}`);
      }
      return node[key];
    }, rootSchema);
}

function validateSchemaValue(value, schemaNode, rootSchema, pointer = "$") {
  if (!schemaNode || typeof schemaNode !== "object") {
    return [];
  }
  if (schemaNode.$ref) {
    return validateSchemaValue(value, resolveSchemaRef(schemaNode.$ref, rootSchema), rootSchema, pointer);
  }
  if (Array.isArray(schemaNode.anyOf)) {
    const variants = schemaNode.anyOf.map((variant) => validateSchemaValue(value, variant, rootSchema, pointer));
    if (variants.some((errors) => errors.length === 0)) {
      return [];
    }
    return [`${pointer} did not match anyOf: ${variants.map((errors) => errors[0]).filter(Boolean).join("; ")}`];
  }
  if (Array.isArray(schemaNode.oneOf)) {
    const matches = schemaNode.oneOf
      .map((variant) => validateSchemaValue(value, variant, rootSchema, pointer))
      .filter((errors) => errors.length === 0);
    return matches.length === 1 ? [] : [`${pointer} matched ${matches.length} oneOf variants`];
  }

  const errors = [];
  const actualType = schemaTypeOf(value);
  if (schemaNode.type) {
    const allowedTypes = Array.isArray(schemaNode.type) ? schemaNode.type : [schemaNode.type];
    if (!allowedTypes.includes(actualType)) {
      errors.push(`${pointer} expected ${allowedTypes.join("|")}, got ${actualType}`);
      return errors;
    }
  }
  if ("const" in schemaNode && value !== schemaNode.const) {
    errors.push(`${pointer} expected const ${JSON.stringify(schemaNode.const)}`);
  }
  if (Array.isArray(schemaNode.enum) && !schemaNode.enum.includes(value)) {
    errors.push(`${pointer} expected one of ${schemaNode.enum.map((entry) => JSON.stringify(entry)).join(", ")}`);
  }
  if (typeof value === "string") {
    if (schemaNode.minLength !== undefined && value.length < schemaNode.minLength) {
      errors.push(`${pointer} length ${value.length} < ${schemaNode.minLength}`);
    }
    if (schemaNode.pattern && !(new RegExp(schemaNode.pattern).test(value))) {
      errors.push(`${pointer} does not match ${schemaNode.pattern}`);
    }
  }
  if (typeof value === "number") {
    if (schemaNode.minimum !== undefined && value < schemaNode.minimum) {
      errors.push(`${pointer} ${value} < ${schemaNode.minimum}`);
    }
    if (schemaNode.maximum !== undefined && value > schemaNode.maximum) {
      errors.push(`${pointer} ${value} > ${schemaNode.maximum}`);
    }
  }
  if (Array.isArray(value)) {
    if (schemaNode.minItems !== undefined && value.length < schemaNode.minItems) {
      errors.push(`${pointer} has ${value.length} item(s), expected at least ${schemaNode.minItems}`);
    }
    if (schemaNode.maxItems !== undefined && value.length > schemaNode.maxItems) {
      errors.push(`${pointer} has ${value.length} item(s), expected at most ${schemaNode.maxItems}`);
    }
    if (schemaNode.items) {
      value.forEach((item, index) => {
        errors.push(...validateSchemaValue(item, schemaNode.items, rootSchema, `${pointer}[${index}]`));
      });
    }
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const properties = schemaNode.properties || {};
    const required = Array.isArray(schemaNode.required) ? schemaNode.required : [];
    for (const field of required) {
      if (!(field in value)) {
        errors.push(`${pointer}.${field} is required`);
      }
    }
    for (const [field, fieldValue] of Object.entries(value)) {
      if (field in properties) {
        errors.push(...validateSchemaValue(fieldValue, properties[field], rootSchema, `${pointer}.${field}`));
      } else if (schemaNode.additionalProperties === false) {
        errors.push(`${pointer}.${field} is not allowed`);
      }
    }
  }
  return errors;
}

function decodeEntities(text) {
  return text
    .replace(/&nbsp;/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, "\"")
    .replace(/&#39;/g, "'")
    .replace(/&#x([0-9a-f]+);/gi, (_, hex) => String.fromCodePoint(Number.parseInt(hex, 16)))
    .replace(/&#([0-9]+);/g, (_, dec) => String.fromCodePoint(Number.parseInt(dec, 10)));
}

function stripTags(html) {
  return decodeEntities(html)
    .replace(/<script[\s\S]*?<\/script>/gi, " ")
    .replace(/<style[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function extractTitle(raw, contentType) {
  if (contentType === "text/html") {
    const title = raw.match(/<title[^>]*>([\s\S]*?)<\/title>/i);
    if (title) {
      return stripTags(title[1]);
    }
    const h1 = raw.match(/<h1[^>]*>([\s\S]*?)<\/h1>/i);
    if (h1) {
      return stripTags(h1[1]);
    }
  }
  return raw.split(/\r?\n/).map((line) => line.trim()).find(Boolean) || "Untitled input";
}

function extractHeadings(raw, maxItems) {
  const items = [];
  const headingPattern = /<h([1-4])([^>]*)>([\s\S]*?)<\/h\1>/gi;
  let match;
  while ((match = headingPattern.exec(raw)) && items.length < maxItems) {
    const attrs = match[2] || "";
    const id = attrs.match(/\sid=["']([^"']+)["']/i)?.[1] || null;
    const text = stripTags(match[3]);
    if (text.length >= 3) {
      items.push({
        kind: "heading",
        level: Number(match[1]),
        text,
        anchor: id,
      });
    }
  }
  return items;
}

function extractHttpTokens(text, maxItems) {
  const tokenPattern = /\b(GET|HEAD|POST|PUT|DELETE|CONNECT|OPTIONS|TRACE|PATCH|HTTP\/[0-9.]+|Content-Type|Content-Length|Cache-Control|Authorization|Location|ETag|Accept)\b/g;
  const seen = new Set();
  const items = [];
  let match;
  while ((match = tokenPattern.exec(text)) && items.length < maxItems) {
    const token = match[1];
    if (seen.has(token)) {
      continue;
    }
    seen.add(token);
    const start = Math.max(0, match.index - 90);
    const end = Math.min(text.length, match.index + token.length + 90);
    items.push({
      kind: "term",
      text: token,
      context: text.slice(start, end).replace(/\s+/g, " ").trim(),
    });
  }
  return items;
}

function extractParagraphs(raw, maxItems) {
  const paragraphs = [];
  const pattern = /<p[^>]*>([\s\S]*?)<\/p>/gi;
  let match;
  while ((match = pattern.exec(raw)) && paragraphs.length < maxItems) {
    const text = stripTags(match[1]);
    if (text.length > 80) {
      paragraphs.push({
        kind: "paragraph",
        text: text.slice(0, 360),
      });
    }
  }
  return paragraphs;
}

function validatePacket(packet, schema) {
  const counts = {
    heading: packet.extraction.items.filter((item) => item.kind === "heading").length,
    term: packet.extraction.items.filter((item) => item.kind === "term").length,
    paragraph: packet.extraction.items.filter((item) => item.kind === "paragraph").length,
  };
  const checks = [
    { name: "has_source_digest", passed: /^sha256:[0-9a-f]{64}$/.test(packet.source.input_digest), detail: packet.source.input_digest },
    { name: "has_schema_digest", passed: /^sha256:[0-9a-f]{64}$/.test(packet.validation.schema_digest), detail: packet.validation.schema_digest },
    { name: "has_minimum_items", passed: Array.isArray(packet.extraction.items) && packet.extraction.items.length >= 8, detail: String(packet.extraction.items.length) },
    {
      name: "summary_counts_match_items",
      passed:
        packet.extraction.summary.item_count === packet.extraction.items.length &&
        packet.extraction.summary.heading_count === counts.heading &&
        packet.extraction.summary.term_count === counts.term &&
        packet.extraction.summary.paragraph_count === counts.paragraph,
      detail: JSON.stringify(counts),
    },
  ];
  packet.validation.checks = checks;
  packet.validation.valid = checks.every((check) => check.passed);
  const schemaErrors = validateSchemaValue(packet, schema, schema);
  checks.push({
    name: "json_schema_validation",
    passed: schemaErrors.length === 0,
    detail: schemaErrors.length === 0 ? "output validates against packets/extraction.schema.json" : schemaErrors.slice(0, 6).join("; "),
  });
  packet.validation.valid = checks.every((check) => check.passed);
  return {
    valid: packet.validation.valid,
    checks,
    schemaErrors,
  };
}

function main() {
  const inputs = readInputs();
  const inputPath = resolveInsidePackage(inputs.input_path, "input_path");
  const schemaPath = resolveInsidePackage(inputs.schema_path, "schema_path");
  const contentType = String(inputs.content_type || "text/html");
  const maxItems = Math.max(8, Math.min(Number(inputs.max_items || 20), 60));
  const sourceUrl = String(inputs.source_url || "").trim();
  if (!sourceUrl) {
    throw new Error("source_url is required");
  }

  const inputBytes = fs.readFileSync(inputPath);
  const raw = inputBytes.toString("utf8");
  const schemaBytes = fs.readFileSync(schemaPath);
  const schema = JSON.parse(schemaBytes.toString("utf8"));
  const plainText = contentType === "text/html" ? stripTags(raw) : raw.replace(/\s+/g, " ").trim();

  const headings = contentType === "text/html" ? extractHeadings(raw, Math.ceil(maxItems / 2)) : [];
  const paragraphs = contentType === "text/html" ? extractParagraphs(raw, 3) : [];
  const terms = extractHttpTokens(plainText, maxItems);
  const items = [...headings, ...paragraphs, ...terms].slice(0, maxItems);

  const extraction = {
    title: extractTitle(raw, contentType),
    summary: {
      item_count: items.length,
      heading_count: items.filter((item) => item.kind === "heading").length,
      term_count: items.filter((item) => item.kind === "term").length,
      paragraph_count: items.filter((item) => item.kind === "paragraph").length,
      text_chars: plainText.length,
    },
    items,
  };

  const packet = {
    schema: "runx/extract.result.v1",
    source: {
      url: sourceUrl,
      content_type: contentType,
      input_path: String(inputs.input_path),
      input_digest: sha256Bytes(inputBytes),
      input_bytes: inputBytes.length,
    },
    extraction,
    validation: {
      schema_id: schema.$id || "runx/extract.result.v1",
      schema_digest: sha256Bytes(schemaBytes),
      valid: false,
      engine: "native-json-schema-subset-v1",
      checks: [],
    },
    provenance: {
      mode: "fixture",
      tool_version: TOOL_VERSION,
      source_kind: "real_public_document",
      output_payload_digest: null,
    },
  };

  const outputPayload = {
    source: packet.source,
    extraction: packet.extraction,
    validation_schema_id: packet.validation.schema_id,
  };
  packet.provenance.output_payload_digest = sha256Text(canonicalJson(outputPayload));
  const validation = validatePacket(packet, schema);
  packet.validation.valid = validation.valid;
  packet.validation.checks = validation.checks;
  const finalSchemaErrors = validateSchemaValue(packet, schema, schema);
  if (finalSchemaErrors.length > 0) {
    packet.validation.valid = false;
    packet.validation.checks.push({
      name: "final_json_schema_validation",
      passed: false,
      detail: finalSchemaErrors.slice(0, 6).join("; "),
    });
  }
  if (!packet.validation.valid) {
    throw new Error(`schema validation failed: ${JSON.stringify(packet.validation.checks)}`);
  }

  packet.artifacts = [
    {
      id: packet.source.input_digest,
      artifact_id: packet.source.input_digest,
      type: "input_fixture",
      artifact_type: "input_fixture",
      label: "RFC 9110 HTML fixture",
      source_url: sourceUrl,
      byte_count: inputBytes.length,
    },
    {
      id: packet.validation.schema_digest,
      artifact_id: packet.validation.schema_digest,
      type: "json_schema",
      artifact_type: "json_schema",
      label: packet.validation.schema_id,
    },
    {
      id: packet.provenance.output_payload_digest,
      artifact_id: packet.provenance.output_payload_digest,
      type: "validated_output",
      artifact_type: "validated_output",
      label: "Structured extraction JSON payload",
    },
  ];
  packet.signal = {
    signal_id: `extract:${packet.source.input_digest}:${packet.provenance.output_payload_digest}`,
    source_events: [
      {
        provider: "rfc-editor",
        source_locator: sourceUrl,
        title: "RFC 9110 HTTP Semantics HTML",
      },
    ],
    artifacts: packet.artifacts,
  };

  process.stdout.write(JSON.stringify(packet));
}

try {
  main();
} catch (error) {
  process.stderr.write(`${JSON.stringify({ error: { message: error.message } })}\n`);
  process.exitCode = 1;
}
