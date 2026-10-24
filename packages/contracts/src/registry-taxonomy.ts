/**
 * The registry taxonomy, projected from `registry/taxonomy.json` by
 * `scripts/generate-registry-taxonomy.mjs`. The hosted registry serves each
 * category's label and mark from here; nothing else defines a category.
 */
import { registryTaxonomy } from "./generated/registry-taxonomy.js";

export type RegistryTaxonomySubject = "skill" | "connector";

export interface RegistryTaxonomyMark {
  /** The `sha256:` digest of `svg`: the mark's content address. */
  readonly digest: string;
  readonly svg: string;
}

export interface RegistryTaxonomyCategory {
  readonly id: string;
  readonly label: string;
  readonly description?: string;
  readonly applies_to?: readonly RegistryTaxonomySubject[];
  readonly surface?: "domain" | "builder";
  readonly mark: RegistryTaxonomyMark;
}

export interface RegistryTaxonomy {
  /** Stored for a skill or connector that declares no category; never declarable. */
  readonly uncategorized: RegistryTaxonomyCategory;
  readonly categories: readonly RegistryTaxonomyCategory[];
}

export { registryTaxonomy };

/** Stored for a skill or connector that declares no category. Never declarable. */
export const UNCATEGORIZED = registryTaxonomy.uncategorized.id;

const categoryById = new Map(registryTaxonomy.categories.map((category) => [category.id, category]));

/** A declared category outside the taxonomy, or of the other subject kind. */
export class RegistryCategoryError extends Error {
  constructor(readonly subject: RegistryTaxonomySubject, readonly category: string) {
    super(`${JSON.stringify(category)} is not a ${subject} category of the registry taxonomy.`);
    this.name = "RegistryCategoryError";
  }
}

/** The categories one kind of subject may declare, in rail order. */
export function registryCategories(subject: RegistryTaxonomySubject): readonly RegistryTaxonomyCategory[] {
  return registryTaxonomy.categories.filter((category) => appliesTo(category, subject));
}

/** A stored category's taxonomy entry, `uncategorized` included. */
export function registryCategory(id: string): RegistryTaxonomyCategory | undefined {
  return id === UNCATEGORIZED ? registryTaxonomy.uncategorized : categoryById.get(id);
}

/**
 * What publication stores: the declared category, exactly as declared and
 * applicable to its kind, or `uncategorized` when none is declared.
 */
export function declaredRegistryCategory(subject: RegistryTaxonomySubject, declared: string | undefined): string {
  if (declared === undefined) return UNCATEGORIZED;
  const category = categoryById.get(declared);
  if (!category || !appliesTo(category, subject)) throw new RegistryCategoryError(subject, declared);
  return declared;
}

function appliesTo(category: RegistryTaxonomyCategory, subject: RegistryTaxonomySubject): boolean {
  return category.applies_to === undefined || category.applies_to.includes(subject);
}
