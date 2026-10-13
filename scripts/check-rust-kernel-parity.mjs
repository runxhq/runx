#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const apiOnly = process.argv.includes("--api-only");
const cargoPublicApiVersion = "0.51.0";
// The rustdoc JSON format moves with nightly, so the snapshot is read through
// one pinned nightly; a floating one would change the gate under an unchanged
// tree. Advance it with cargoPublicApiVersion.
const publicApiToolchain = "nightly-2026-10-13";

const commands = apiOnly
  ? [checkPublicApiSnapshot]
  : [
      checkCargo,
      checkTooling,
      () => run("cargo", ["fmt", "--manifest-path", "crates/Cargo.toml", "--all", "--check"]),
      () => run("cargo", ["clippy", "--manifest-path", "crates/Cargo.toml", "--workspace", "--all-targets", "--", "-D", "warnings"]),
      () => run("cargo", ["test", "--manifest-path", "crates/Cargo.toml", "--workspace"]),
      () => run("node", ["scripts/check-rust-crate-graph.mjs"]),
      // Keep these strings contiguous for scafld source checks:
      // cargo deny
      // cargo public-api
      () => run("cargo", ["deny", "--manifest-path", "crates/Cargo.toml", "check", "bans", "licenses", "sources"]),
      checkPublicApiSnapshot,
    ];

for (const command of commands) {
  command();
}

function checkCargo() {
  const result = spawnSync("cargo", ["--version"], { encoding: "utf8" });
  if (result.status === 0) {
    return;
  }
  console.error("cargo is not installed. Install Rust with rustup: https://rustup.rs/");
  console.error("After installing Rust, rerun: node scripts/check-rust-kernel-parity.mjs");
  process.exit(1);
}

function checkTooling() {
  const missing = [];
  if (spawnSync("cargo", ["deny", "--version"], { encoding: "utf8" }).status !== 0) {
    missing.push("cargo-deny");
  }
  if (!cargoPublicApiInstalled()) {
    missing.push("cargo-public-api");
  }
  if (missing.length === 0) {
    checkCargoPublicApiVersion();
    return;
  }
  console.error(`missing Cargo parity tool(s): ${missing.join(", ")}`);
  console.error("Install optional Rust parity tools with:");
  console.error(`  cargo install cargo-deny && cargo install cargo-public-api --version ${cargoPublicApiVersion}`);
  process.exit(1);
}

function checkPublicApiSnapshot() {
  checkCargo();
  if (!cargoPublicApiInstalled()) {
    console.error("missing Cargo parity tool: cargo-public-api");
    console.error(`Install it with: cargo install cargo-public-api --version ${cargoPublicApiVersion}`);
    process.exit(1);
  }
  checkCargoPublicApiVersion();
  ensurePublicApiToolchain();

  const result = spawnSync(
    "cargo",
    [`+${publicApiToolchain}`, "public-api", "--manifest-path", "crates/runx-core/Cargo.toml", "-sss"],
    { encoding: "utf8" },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    process.stderr.write(result.stdout);
    process.exit(result.status ?? 1);
  }

  const expectedPath = "crates/runx-core/api-snapshot.txt";
  const expected = readFileSync(expectedPath, "utf8");
  const actual = result.stdout.endsWith("\n") ? result.stdout : `${result.stdout}\n`;
  if (actual === expected) {
    return;
  }

  console.error(`${expectedPath} is stale; regenerate with:`);
  console.error(`  cargo +${publicApiToolchain} public-api --manifest-path crates/runx-core/Cargo.toml -sss > crates/runx-core/api-snapshot.txt`);
  process.exit(1);
}

function ensurePublicApiToolchain() {
  const installed = spawnSync("rustup", ["toolchain", "list", "--quiet"], { encoding: "utf8" }).stdout ?? "";
  if (installed.split(/\r?\n/u).some((line) => line.trim().startsWith(`${publicApiToolchain}-`))) {
    return;
  }
  run("rustup", ["toolchain", "install", publicApiToolchain, "--profile", "minimal"]);
}

function cargoPublicApiInstalled() {
  return spawnSync("cargo", ["public-api", "--version"], { encoding: "utf8" }).status === 0;
}

function checkCargoPublicApiVersion() {
  const result = spawnSync("cargo", ["public-api", "--version"], { encoding: "utf8" });
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    process.exit(result.status ?? 1);
  }

  const actual = result.stdout.trim();
  const expected = `cargo-public-api ${cargoPublicApiVersion}`;
  if (actual === expected) {
    return;
  }

  console.error(`cargo-public-api version mismatch: expected ${expected}, got ${actual}`);
  console.error(`Install the pinned snapshot tool with: cargo install cargo-public-api --version ${cargoPublicApiVersion}`);
  process.exit(1);
}

function run(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
