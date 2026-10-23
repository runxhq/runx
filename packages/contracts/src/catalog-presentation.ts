import { parseDocument } from "yaml";

import { declaredRegistryCategory } from "./registry-taxonomy.js";

/** Where a package carries its optional descriptive catalog content. */
export const CATALOG_PRESENTATION_PATH = "references/catalog.md";

/** A child skill a method step can name: a sibling (`name`, `../name`) or `publisher/name`. */
const SKILL_SELECTOR = /^(?:(?:\.\.\/)?[a-z0-9][a-z0-9._-]*|[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._-]*)$/u;

/** The skill a method step's `skill` names: a sibling is under the same publisher. */
export function composedSkillId(skill: string, publisher: string): string {
  const sibling = skill.startsWith("../") ? skill.slice(3) : skill;
  return sibling.includes('/') ? sibling : `${publisher}/${sibling}`;
}
export const CATALOG_PRESENTATION_SCHEMA = "runx.catalog-presentation.v1";

const MAX_DEPTH = 4;
const MAX_TEXT = 600;
const MAX_ITEMS = 12;
/** The overview is a page's introduction, not a manual: the manual is SKILL.md. */
const MAX_OVERVIEW = 4_000;
const RELATIONSHIP_KINDS = ["evidence_source", "handoff", "ecosystem"] as const;

export type RegistryRelationshipKind = (typeof RELATIONSHIP_KINDS)[number];

/**
 * A package's descriptive presentation: narrative facts its author publishes
 * with the package. It never carries trust, versions, counts, pricing or
 * classification; those come from their own owners.
 */
export interface RegistryCatalogPresentation {
  readonly schema: typeof CATALOG_PRESENTATION_SCHEMA;
  readonly title?: string;
  /** The one line a catalog card shows. */
  readonly summary?: string;
  /** The authored overview: the file's Markdown body. */
  readonly overview?: string;
  readonly method?: readonly { readonly title: string; readonly description: string; readonly skill?: string; readonly runner?: string }[];
  readonly inputs?: readonly { readonly label: string; readonly description?: string; readonly input?: string }[];
  readonly outputs?: readonly { readonly label: string; readonly description?: string }[];
  readonly stops?: { readonly title: string; readonly description?: string };
  readonly example?: { readonly title: string; readonly path: string };
  readonly relationships?: readonly {
    readonly kind: RegistryRelationshipKind;
    readonly id: string;
    readonly label: string;
    readonly reference?: string;
  }[];
}

/** An invalid presentation file: a publication error naming the exact field. */
export class RegistryPresentationError extends Error {
  constructor(readonly field: string, message: string) {
    super(`${CATALOG_PRESENTATION_PATH} ${field}: ${message}`);
    this.name = "RegistryPresentationError";
  }
}

/**
 * Validates `references/catalog.md` against the package it travels in. An
 * absent file is valid; an invalid one is a publication error, never ignored.
 * Composition references are checked against native dependency metadata when
 * the package projection carries it; until then their form is checked here.
 */
export function readCatalogPresentation(
  source: string,
  context: {
    readonly packagePaths: ReadonlySet<string>;
    readonly runnerNames: readonly string[];
    /** The input names the package declares: an `input` must name one. */
    readonly inputNames: readonly string[];
    /** The child skills the package composes, as written: a method step's `skill` must name one. */
    readonly skills: readonly string[];
  },
): RegistryCatalogPresentation {
  const { frontmatter, body } = splitFrontmatter(source);
  const document = parseDocument(frontmatter, { uniqueKeys: true, prettyErrors: false });
  if (document.errors.length > 0) throw new RegistryPresentationError("frontmatter", document.errors[0]!.message);
  let value: unknown;
  try {
    // Aliases are refused outright: presentation is plain descriptive data.
    value = document.toJS({ maxAliasCount: 0 });
  } catch (error) {
    throw new RegistryPresentationError("frontmatter", error instanceof Error ? error.message : String(error));
  }
  depthWithin(value, "frontmatter", 0);
  const record = object(value, "frontmatter");
  allowOnly(record, "frontmatter", ["schema", "title", "summary", "method", "inputs", "outputs", "stops", "example", "relationships"]);
  if (record.schema !== CATALOG_PRESENTATION_SCHEMA) {
    throw new RegistryPresentationError("schema", `must be ${CATALOG_PRESENTATION_SCHEMA}`);
  }
  const overview = body.trim();
  if (overview.length > MAX_OVERVIEW) throw new RegistryPresentationError("overview", `is longer than ${MAX_OVERVIEW} characters`);
  const method = list(record.method, "method", (entry, field) => {
    const item = object(entry, field);
    allowOnly(item, field, ["title", "description", "skill", "runner"]);
    const runner = optionalText(item.runner, `${field}.runner`);
    if (runner !== undefined && !context.runnerNames.includes(runner)) {
      throw new RegistryPresentationError(`${field}.runner`, `names no runner this package declares (${runner})`);
    }
    const skill = optionalText(item.skill, `${field}.skill`);
    if (skill !== undefined && !SKILL_SELECTOR.test(skill)) {
      throw new RegistryPresentationError(`${field}.skill`, "must be a sibling (name or ../name) or publisher/name");
    }
    if (skill !== undefined && !context.skills.includes(skill)) {
      throw new RegistryPresentationError(`${field}.skill`, `names a skill the package does not compose (${skill})`);
    }
    return {
      title: text(item.title, `${field}.title`),
      description: text(item.description, `${field}.description`),
      ...(skill ? { skill } : {}),
      ...(runner ? { runner } : {}),
    };
  });
  const inputs = list(record.inputs, "inputs", (entry, field) => {
    const item = object(entry, field);
    allowOnly(item, field, ["label", "description", "input"]);
    const input = optionalText(item.input, `${field}.input`);
    if (input !== undefined && !/^[A-Za-z_][A-Za-z0-9_]*$/u.test(input)) {
      throw new RegistryPresentationError(`${field}.input`, "names a runner input by its plain name");
    }
    if (input !== undefined && !context.inputNames.includes(input)) {
      throw new RegistryPresentationError(`${field}.input`, `names an input the package does not declare (${input})`);
    }
    const description = optionalText(item.description, `${field}.description`);
    return { label: text(item.label, `${field}.label`), ...(description ? { description } : {}), ...(input ? { input } : {}) };
  });
  const outputs = list(record.outputs, "outputs", (entry, field) => {
    const item = object(entry, field);
    allowOnly(item, field, ["label", "description"]);
    const description = optionalText(item.description, `${field}.description`);
    return { label: text(item.label, `${field}.label`), ...(description ? { description } : {}) };
  });
  const stops = record.stops === undefined ? undefined : (() => {
    const item = object(record.stops, "stops");
    allowOnly(item, "stops", ["title", "description"]);
    const description = optionalText(item.description, "stops.description");
    return { title: text(item.title, "stops.title"), ...(description ? { description } : {}) };
  })();
  const example = record.example === undefined ? undefined : (() => {
    const item = object(record.example, "example");
    allowOnly(item, "example", ["title", "path"]);
    const path = packagePath(item.path, "example.path", context.packagePaths);
    if (!path.startsWith("references/") || !path.endsWith(".md")) {
      throw new RegistryPresentationError("example.path", "must be a Markdown document under references/");
    }
    return { title: text(item.title, "example.title"), path };
  })();
  const relationships = list(record.relationships, "relationships", (entry, field) => {
    const item = object(entry, field);
    allowOnly(item, field, ["kind", "id", "label", "reference"]);
    if (!RELATIONSHIP_KINDS.includes(item.kind as RegistryRelationshipKind)) {
      throw new RegistryPresentationError(`${field}.kind`, `must be one of ${RELATIONSHIP_KINDS.join(", ")}`);
    }
    const id = text(item.id, `${field}.id`);
    if (!/^[a-z0-9][a-z0-9._/-]*$/u.test(id)) throw new RegistryPresentationError(`${field}.id`, "must be a canonical identifier");
    const reference = item.reference === undefined ? undefined : packagePath(item.reference, `${field}.reference`, context.packagePaths);
    return {
      kind: item.kind as RegistryRelationshipKind,
      id,
      label: text(item.label, `${field}.label`),
      ...(reference ? { reference } : {}),
    };
  });
  const title = record.title === undefined ? undefined : declaredLine(record.title, "title", MAX_TITLE);
  const summary = record.summary === undefined ? undefined : declaredLine(record.summary, "summary", MAX_SUMMARY);
  return {
    schema: CATALOG_PRESENTATION_SCHEMA,
    ...(title ? { title } : {}),
    ...(summary ? { summary } : {}),
    ...(overview ? { overview } : {}),
    ...(method.length ? { method } : {}),
    ...(inputs.length ? { inputs } : {}),
    ...(outputs.length ? { outputs } : {}),
    ...(stops ? { stops } : {}),
    ...(example ? { example } : {}),
    ...(relationships.length ? { relationships } : {}),
  };
}

/** Longest declared title and summary, in Unicode scalar values after NFC. */
const MAX_TITLE = 100;
const MAX_SUMMARY = 160;

/**
 * A declared display line (a title or a card summary): single-line plain text
 * after NFC, with no control characters and no Markdown or HTML markup.
 */
function declaredLine(value: unknown, field: "title" | "summary", max: number): string {
  if (typeof value !== "string") throw new RegistryPresentationError(field, "must be text");
  const line = value.normalize("NFC").trim();
  const length = [...line].length;
  if (length === 0 || length > max) throw new RegistryPresentationError(field, `must be 1 to ${max} characters`);
  if (/[\p{Cc}\p{Cf}\u2028\u2029]/u.test(line)) throw new RegistryPresentationError(field, "must be one line of plain text");
  if (/[<>`*[\]]|^#/u.test(line)) throw new RegistryPresentationError(field, "must be plain text, without Markdown or HTML markup");
  return line;
}

function splitFrontmatter(source: string): { readonly frontmatter: string; readonly body: string } {
  const match = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/u.exec(source);
  return match ? { frontmatter: match[1]!, body: match[2]! } : { frontmatter: "", body: source };
}

function depthWithin(value: unknown, field: string, depth: number): void {
  if (depth > MAX_DEPTH) throw new RegistryPresentationError(field, `nests deeper than ${MAX_DEPTH} levels`);
  if (Array.isArray(value)) value.forEach((entry, index) => depthWithin(entry, `${field}[${index}]`, depth + 1));
  else if (value && typeof value === "object") {
    for (const [key, entry] of Object.entries(value)) depthWithin(entry, `${field}.${key}`, depth + 1);
  }
}

function object(value: unknown, field: string): Readonly<Record<string, unknown>> {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new RegistryPresentationError(field, "must be a mapping");
  return value as Readonly<Record<string, unknown>>;
}

function allowOnly(record: Readonly<Record<string, unknown>>, field: string, keys: readonly string[]): void {
  const unknown = Object.keys(record).filter((key) => !keys.includes(key));
  if (unknown.length > 0) throw new RegistryPresentationError(field, `has unknown fields: ${unknown.join(", ")}`);
}

function text(value: unknown, field: string): string {
  if (typeof value !== "string" || value.trim().length === 0) throw new RegistryPresentationError(field, "must be non-empty text");
  const trimmed = value.trim();
  if (trimmed.length > MAX_TEXT) throw new RegistryPresentationError(field, `is longer than ${MAX_TEXT} characters`);
  return trimmed;
}

function optionalText(value: unknown, field: string): string | undefined {
  return value === undefined ? undefined : text(value, field);
}

function list<T>(value: unknown, field: string, read: (entry: unknown, field: string) => T): readonly T[] {
  if (value === undefined) return [];
  if (!Array.isArray(value)) throw new RegistryPresentationError(field, "must be a list");
  if (value.length > MAX_ITEMS) throw new RegistryPresentationError(field, `has more than ${MAX_ITEMS} entries`);
  return value.map((entry, index) => read(entry, `${field}[${index}]`));
}

function packagePath(value: unknown, field: string, packagePaths: ReadonlySet<string>): string {
  const path = text(value, field);
  if (path.startsWith("/") || path.split("/").some((part) => part === ".." || part === "." || part === "")) {
    throw new RegistryPresentationError(field, "must be a package-relative path");
  }
  if (!packagePaths.has(path)) throw new RegistryPresentationError(field, `names a file the package does not contain (${path})`);
  return path;
}

/** The part of a skill package the public catalog rule reads. */
export interface PublicSkillCatalogMaterial {
  readonly metadata: {
    readonly name: string;
    readonly catalog_visibility?: string;
    readonly category?: string;
    readonly runner_names: readonly string[];
    readonly input_names: readonly string[];
    readonly skills: readonly string[];
  };
  /** The package's files, at normalized package-relative paths. */
  readonly packageFiles?: readonly { readonly path: string; readonly content: string }[];
}

/**
 * New public versions need authored catalog facts: a taxonomy category and a
 * catalog page with a display title and a summary. The registry refuses a
 * public skill without them; the skill check applies the same rule first.
 */
export function assertPublicSkillCatalog(material: PublicSkillCatalogMaterial): void {
  const metadata = material.metadata;
  if (metadata.catalog_visibility !== "public") return;
  if (!metadata.category) {
    throw new Error("run.yaml catalog.category is required for a public skill.");
  }
  declaredRegistryCategory("skill", metadata.category);
  const packageFiles = material.packageFiles ?? [];
  const presentationFile = packageFiles.find((file) => file.path === CATALOG_PRESENTATION_PATH);
  const presentation = presentationFile && readCatalogPresentation(presentationFile.content, {
    packagePaths: new Set(packageFiles.map((file) => file.path)),
    runnerNames: metadata.runner_names,
    inputNames: metadata.input_names,
    skills: metadata.skills,
  });
  if (!presentation?.title) {
    throw new Error(`${CATALOG_PRESENTATION_PATH} must declare a title for a public skill.`);
  }
  if (presentation.title === metadata.name) {
    throw new Error(`${CATALOG_PRESENTATION_PATH} title must be an authored display name, not the skill slug.`);
  }
  if (!presentation.summary) {
    throw new Error(`${CATALOG_PRESENTATION_PATH} must declare a summary for a public skill.`);
  }
}
