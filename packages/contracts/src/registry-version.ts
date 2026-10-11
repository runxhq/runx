/**
 * The one grammar for a registry version label, mirrored from
 * `runx-contracts::registry_version` and held to the same vectors
 * (`fixtures/contracts/registry-version`). A label starts with an ASCII letter
 * or digit, so it never reads as a flag, a path or a shell word.
 */
export const REGISTRY_VERSION_PATTERN = "^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$";

const registryVersion = new RegExp(REGISTRY_VERSION_PATTERN, "u");

export function isRegistryVersion(value: string): boolean {
  return registryVersion.test(value);
}
