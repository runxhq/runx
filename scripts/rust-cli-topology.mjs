import { readFileSync } from "node:fs";
import path from "node:path";

/**
 * @typedef {object} RustCliPlatformSpec
 * @property {string} key
 * @property {string} package
 * @property {"darwin" | "linux" | "win32"} os
 * @property {"arm64" | "x64"} cpu
 * @property {string} method
 * @property {string} rustTarget
 * @property {"tar.gz" | "zip"} archiveExtension
 * @property {"bin/runx" | "bin/runx.exe"} binary
 * @property {"bin/runx-js-worker" | "bin/runx-js-worker.exe"} worker
 * @property {"runx" | "runx.exe"} binaryName
 * @property {"runx-js-worker" | "runx-js-worker.exe"} workerName
 */

/**
 * @param {string} workspaceRoot
 * @returns {readonly RustCliPlatformSpec[]}
 */
export function loadRustCliPlatforms(workspaceRoot) {
  const topologyPath = path.join(workspaceRoot, "packages", "cli", "native", "supported-platforms.json");
  const topology = JSON.parse(readFileSync(topologyPath, "utf8"));
  if (topology.schema !== "runx.rust_cli_selector_topology.v1" || !topology.nativePackages) {
    throw new Error("Rust CLI selector topology is missing or unsupported");
  }
  return Object.entries(topology.nativePackages).map(([key, value]) => {
    const entry = value && typeof value === "object" ? value : {};
    if (
      !nonEmpty(entry.package)
      || !isPlatformOs(entry.os)
      || !isPlatformCpu(entry.cpu)
      || !nonEmpty(entry.method)
      || !nonEmpty(entry.rustTarget)
      || !isArchiveExtension(entry.archiveExtension)
      || !isBinary(entry.binary)
      || !isWorker(entry.worker)
    ) {
      throw new Error(`Rust CLI selector topology entry ${key} is incomplete or invalid`);
    }
    return {
      key,
      package: entry.package,
      os: entry.os,
      cpu: entry.cpu,
      method: entry.method,
      rustTarget: entry.rustTarget,
      archiveExtension: entry.archiveExtension,
      binary: entry.binary,
      worker: entry.worker,
      binaryName: entry.binary === "bin/runx" ? "runx" : "runx.exe",
      workerName: entry.worker === "bin/runx-js-worker" ? "runx-js-worker" : "runx-js-worker.exe",
    };
  });
}

/** @param {readonly RustCliPlatformSpec[]} platforms @param {string} key */
export function rustCliPlatformForKey(platforms, key) {
  const platform = platforms.find((entry) => entry.key === key);
  if (!platform) {
    throw new Error(`unsupported Rust CLI package platform: ${key}`);
  }
  return platform;
}

/** @param {readonly RustCliPlatformSpec[]} platforms @param {NodeJS.Platform} os @param {string} cpu */
export function rustCliPlatformKey(platforms, os, cpu) {
  const platform = platforms.find((entry) => entry.os === os && entry.cpu === cpu);
  if (!platform) {
    throw new Error(`unsupported Rust CLI package platform: ${os}/${cpu}`);
  }
  return platform.key;
}

/** @param {string} selectorPackage @param {string} platform */
export function nativeRustCliPackageName(selectorPackage, platform) {
  return `${selectorPackage}-${platform}`;
}

/** @param {unknown} value */
export function isRustCliSignatureEntry(value) {
  if (!value || typeof value !== "object") {
    return false;
  }
  const entry = /** @type {{ kind?: unknown, value?: unknown }} */ (value);
  return nonEmpty(entry.kind) && nonEmpty(entry.value);
}

/** @param {unknown} value */
function nonEmpty(value) {
  return typeof value === "string" && value.trim() !== "";
}

/** @param {unknown} value @returns {value is RustCliPlatformSpec["os"]} */
function isPlatformOs(value) {
  return value === "darwin" || value === "linux" || value === "win32";
}

/** @param {unknown} value @returns {value is RustCliPlatformSpec["cpu"]} */
function isPlatformCpu(value) {
  return value === "arm64" || value === "x64";
}

/** @param {unknown} value @returns {value is RustCliPlatformSpec["archiveExtension"]} */
function isArchiveExtension(value) {
  return value === "tar.gz" || value === "zip";
}

/** @param {unknown} value @returns {value is RustCliPlatformSpec["binary"]} */
function isBinary(value) {
  return value === "bin/runx" || value === "bin/runx.exe";
}

/** @param {unknown} value @returns {value is RustCliPlatformSpec["worker"]} */
function isWorker(value) {
  return value === "bin/runx-js-worker" || value === "bin/runx-js-worker.exe";
}
