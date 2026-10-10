import { readFileSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const releaseTopology = JSON.parse(
  readFileSync(path.join(workspaceRoot, "packages", "cli", "native", "supported-platforms.json"), "utf8"),
);
const TARGETS = {
  darwinArm64: rustTarget("darwin-arm64"),
  darwinX64: rustTarget("darwin-x64"),
  linuxArm64: rustTarget("linux-arm64"),
  linuxX64: rustTarget("linux-x64"),
  winX64: rustTarget("win32-x64"),
};

const options = parseArgs(process.argv.slice(2));
const manifest = JSON.parse(readFileSync(options.input, "utf8"));
const failures = [];
expectEqual("release homepage", manifest.homepage, "https://runx.ai");

const archiveEntriesByTarget = new Map();
for (const target of Object.values(TARGETS)) {
  const artifact = manifest.artifacts?.[target];
  if (!artifact?.file) {
    failures.push(`missing artifact metadata for ${target}`);
    continue;
  }
  const archivePath = path.join(options.archives, artifact.file);
  const entries = listArchiveEntries(archivePath);
  archiveEntriesByTarget.set(target, entries);
  const binary = target.includes("windows")
    ? `${archiveStem(manifest.version, target)}/runx.exe`
    : `${archiveStem(manifest.version, target)}/runx`;
  const worker = target.includes("windows")
    ? `${archiveStem(manifest.version, target)}/runx-js-worker.exe`
    : `${archiveStem(manifest.version, target)}/runx-js-worker`;
  if (!entries.has(binary)) {
    failures.push(`${artifact.file} does not contain ${binary}`);
  }
  if (!entries.has(worker)) {
    failures.push(`${artifact.file} does not contain ${worker}`);
  }
}

const scoop = JSON.parse(readFileSync(path.join(options.channels, "scoop", "runx.json"), "utf8"));
expectEqual("scoop homepage", scoop.homepage, manifest.homepage);
const scoopArch = scoop.architecture?.["64bit"];
const winStem = archiveStem(manifest.version, TARGETS.winX64);
expectEqual("scoop architecture.64bit.extract_dir", scoopArch?.extract_dir, winStem);
expectEqual("scoop architecture.64bit.bin", scoopArch?.bin, "runx.exe");
expectEqual("scoop autoupdate extract_dir", scoop.autoupdate?.architecture?.["64bit"]?.extract_dir, `runx-$version-${TARGETS.winX64}`);

const wingetVersion = readFileSync(path.join(options.channels, "winget", "runxhq.runx.yaml"), "utf8");
expectIncludes("winget version manifest type", wingetVersion, "ManifestType: version");
expectIncludes("winget default locale", wingetVersion, "DefaultLocale: en-US");

const wingetLocale = readFileSync(path.join(options.channels, "winget", "runxhq.runx.locale.en-US.yaml"), "utf8");
expectIncludes("winget homepage", wingetLocale, `PackageUrl: ${manifest.homepage}`);
expectIncludes("winget locale manifest type", wingetLocale, "ManifestType: defaultLocale");
expectIncludes("winget locale package name", wingetLocale, "PackageName: runx");

const wingetInstaller = readFileSync(path.join(options.channels, "winget", "runxhq.runx.installer.yaml"), "utf8");
expectIncludes("winget installer manifest type", wingetInstaller, "ManifestType: installer");
expectIncludes("winget RelativeFilePath", wingetInstaller, `RelativeFilePath: ${winStem}\\runx.exe`);
expectIncludes(
  "winget root NestedInstallerFiles",
  wingetInstaller,
  [
    "NestedInstallerFiles:",
    `  - RelativeFilePath: ${winStem}\\runx.exe`,
    "    PortableCommandAlias: runx",
  ].join("\n"),
);

const aur = readFileSync(path.join(options.channels, "aur", "PKGBUILD"), "utf8");
expectIncludes("aur homepage", aur, `url="${manifest.homepage}"`);
expectIncludes("aur package identity", aur, "pkgname=runxhq-bin");
const aurSrcinfo = readFileSync(path.join(options.channels, "aur", ".SRCINFO"), "utf8");
expectIncludes("aur source info version", aurSrcinfo, `\tpkgver = ${manifest.version}`);
expectIncludes("aur source info homepage", aurSrcinfo, `\turl = ${manifest.homepage}`);
for (const target of [TARGETS.linuxX64, TARGETS.linuxArm64]) {
  expectIncludes("aur source info archive", aurSrcinfo, manifest.artifacts[target].file);
  expectIncludes("aur source info checksum", aurSrcinfo, manifest.artifacts[target].sha256);
}
expectIncludes("aur x86_64 package path", aur, `runx-\${pkgver}-\${target}/runx`);
expectIncludes("aur x86_64 target branch", aur, `x86_64) target="${TARGETS.linuxX64}" ;;`);
expectIncludes("aur aarch64 target branch", aur, `aarch64) target="${TARGETS.linuxArm64}" ;;`);

const homebrew = readFileSync(path.join(options.channels, "homebrew", "runx.rb"), "utf8");
expectIncludes("homebrew homepage", homebrew, `homepage "${manifest.homepage}"`);
expectIncludes("homebrew nested archive install", homebrew, 'bin.install Dir["*/runx"].first => "runx"');
expectIncludes("homebrew worker install", homebrew, 'bin.install Dir["*/runx-js-worker"].first => "runx-js-worker"');
expectIncludes("winget worker file", wingetInstaller, `  - RelativeFilePath: ${winStem}\\runx-js-worker.exe`);
expectIncludes("aur worker install", aur, 'install -Dm755 "runx-${pkgver}-${target}/runx-js-worker" "$pkgdir/usr/bin/runx-js-worker"');

const chocolatey = readFileSync(path.join(options.channels, "chocolatey", "runx.nuspec"), "utf8");
expectIncludes("chocolatey version", chocolatey, `<version>${manifest.version}</version>`);
expectIncludes("chocolatey homepage", chocolatey, `<projectUrl>${manifest.homepage}</projectUrl>`);
const chocolateyInstall = readFileSync(path.join(options.channels, "chocolatey", "tools", "chocolateyinstall.ps1"), "utf8");
expectIncludes("chocolatey archive", chocolateyInstall, `-Url '${archiveUrl(TARGETS.winX64)}'`);
expectIncludes("chocolatey archive checksum", chocolateyInstall, `-Checksum '${manifest.artifacts[TARGETS.winX64].sha256}'`);
expectIncludes("chocolatey worker", chocolateyInstall, "runx-js-worker.exe");
const chocolateyUninstall = readFileSync(path.join(options.channels, "chocolatey", "tools", "chocolateyuninstall.ps1"), "utf8");
expectIncludes("chocolatey shim cleanup", chocolateyUninstall, "Uninstall-BinFile -Name 'runx'");

const macports = readFileSync(path.join(options.channels, "macports", "Portfile"), "utf8");
expectIncludes("macports homepage", macports, `homepage            ${manifest.homepage}`);
for (const target of [TARGETS.darwinArm64, TARGETS.darwinX64]) {
  expectIncludes("macports archive", macports, manifest.artifacts[target].file);
  expectIncludes("macports checksum", macports, manifest.artifacts[target].sha256);
  expectIncludes("macports archive root", macports, `worksrcdir      ${archiveStem(manifest.version, target)}`);
}
expectIncludes("macports worker install", macports, "$worksrcpath/runx-js-worker $destroot$prefix/bin/runx-js-worker");

if (failures.length > 0) {
  console.error(JSON.stringify({ status: "failed", failures }, null, 2));
  process.exit(1);
}

console.log(JSON.stringify({ status: "ok", checked: Object.values(TARGETS).length, version: manifest.version }, null, 2));

function archiveStem(version, target) {
  return `runx-${version}-${target}`;
}

function archiveUrl(target) {
  return `https://github.com/${manifest.repo}/releases/download/${manifest.tag}/${manifest.artifacts[target].file}`;
}

function rustTarget(platform) {
  const target = releaseTopology.nativePackages?.[platform]?.rustTarget;
  if (!target) {
    throw new Error(`release platform topology is missing rustTarget for ${platform}`);
  }
  return target;
}

function listArchiveEntries(archivePath) {
  if (archivePath.endsWith(".zip")) {
    const result = spawnSync("python3", [
      "-c",
      "import json,sys,zipfile; print(json.dumps(zipfile.ZipFile(sys.argv[1]).namelist()))",
      archivePath,
    ], { encoding: "utf8" });
    if (result.status !== 0) {
      throw new Error(`could not list zip ${archivePath}: ${result.stderr || result.stdout}`);
    }
    return new Set(JSON.parse(result.stdout));
  }
  const result = spawnSync("tar", ["-tzf", archivePath], { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`could not list tar ${archivePath}: ${result.stderr || result.stdout}`);
  }
  return new Set(result.stdout.trim().split(/\r?\n/u).filter(Boolean));
}

function expectEqual(label, actual, expected) {
  if (actual !== expected) {
    failures.push(`${label} is ${JSON.stringify(actual)}, expected ${JSON.stringify(expected)}`);
  }
}

function expectIncludes(label, haystack, needle) {
  if (!haystack.includes(needle)) {
    failures.push(`${label} does not include ${JSON.stringify(needle)}`);
  }
}

function parseArgs(argv) {
  const parsed = { input: "", channels: "", archives: "" };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--input") {
      parsed.input = argv[++index] ?? "";
      continue;
    }
    if (arg === "--channels") {
      parsed.channels = argv[++index] ?? "";
      continue;
    }
    if (arg === "--archives") {
      parsed.archives = argv[++index] ?? "";
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  for (const [key, value] of Object.entries(parsed)) {
    if (!value) {
      throw new Error(`--${key} is required`);
    }
  }
  return parsed;
}
