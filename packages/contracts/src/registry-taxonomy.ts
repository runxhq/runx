/**
 * The registry taxonomy, projected from `registry/taxonomy.json` by
 * `scripts/generate-registry-taxonomy.mjs`. The hosted registry serves each
 * category's label and mark from here; nothing else defines a category.
 */
export type RegistryTaxonomySubject = "skill" | "connector";

export interface RegistryTaxonomyMark {
  readonly sha256: string;
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

export { registryTaxonomy } from "./generated/registry-taxonomy.js";
