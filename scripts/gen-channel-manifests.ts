import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Generates package-manager manifests (Homebrew, Scoop, winget, AUR, Chocolatey) for a
// release from one input: the version plus the per-target release-archive
// checksums. The GitHub Release is the hub; every manifest points at its
// archives by URL + sha256. Run after the build job has produced archives and
// a checksums map, before the per-channel push steps.

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));

interface Artifact {
  readonly file: string;
  readonly sha256: string;
}

interface Manifest {
  readonly version: string;
  readonly repo: string; // owner/name on GitHub
  readonly tag: string; // e.g. cli-v0.6.0
  readonly homepage: string;
  readonly description: string;
  readonly artifacts: Record<string, Artifact>; // keyed by rust target triple
  readonly source: { readonly url: string; readonly sha256: string; readonly root: string }; // GitHub's archive of the tag
}

const releaseTopology = JSON.parse(
  readFileSync(path.join(workspaceRoot, "packages", "cli", "native", "supported-platforms.json"), "utf8"),
) as { readonly nativePackages?: Record<string, { readonly rustTarget?: string }> };
const TARGETS = {
  darwinArm64: rustTarget("darwin-arm64"),
  darwinX64: rustTarget("darwin-x64"),
  linuxArm64: rustTarget("linux-arm64"),
  linuxX64: rustTarget("linux-x64"),
  winX64: rustTarget("win32-x64"),
} as const;

// AUR's runx builds the release from source: an AUR package of prebuilt
// binaries must end in -bin, and runx-bin is an unrelated project's runx.
// The PKGBUILD names its source through pkgver, so it must expand to exactly
// the archive the release input hashed.
const AUR_SOURCE_URL = "https://github.com/runxhq/runx/archive/refs/tags/cli-v$pkgver.tar.gz";
const AUR_SOURCE_ROOT = "$pkgname-cli-v$pkgver";

const options = parseArgs(process.argv.slice(2));
const manifest = JSON.parse(readFileSync(path.resolve(workspaceRoot, options.input), "utf8")) as Manifest;
const outDir = path.resolve(workspaceRoot, options.outDir);

const written: string[] = [];
write("homebrew/runx.rb", renderHomebrew(manifest));
write("scoop/runx.json", renderScoop(manifest));
for (const file of renderWinget(manifest)) {
  write(file.path, file.contents);
}
write("aur/PKGBUILD", renderPkgbuild(manifest));
write("aur/.SRCINFO", renderAurSrcinfo(manifest));
write("chocolatey/runx.nuspec", renderChocolateyNuspec(manifest));
write("chocolatey/tools/chocolateyinstall.ps1", renderChocolateyInstall(manifest));
write("chocolatey/tools/chocolateyuninstall.ps1", renderChocolateyUninstall());

console.log(JSON.stringify({ status: "generated", version: manifest.version, files: written }, null, 2));

function archiveUrl(m: Manifest, target: string): string {
  return `https://github.com/${m.repo}/releases/download/${m.tag}/${artifact(m, target).file}`;
}

function rustTarget(platform: string): string {
  const target = releaseTopology.nativePackages?.[platform]?.rustTarget;
  if (!target) {
    throw new Error(`release platform topology is missing rustTarget for ${platform}`);
  }
  return target;
}

function archiveStem(m: Manifest, target: string): string {
  return `runx-${m.version}-${target}`;
}

function windowsArchivePath(m: Manifest, target: string, file: string): string {
  return `${archiveStem(m, target)}\\${file}`;
}

function artifact(m: Manifest, target: string): Artifact {
  const entry = m.artifacts[target];
  if (!entry) {
    throw new Error(`missing release artifact for target ${target}`);
  }
  return entry;
}

function renderHomebrew(m: Manifest): string {
  // A binary cask-style formula: download the prebuilt archive per platform.
  return `# typed: false
# frozen_string_literal: true

class Runx < Formula
  desc "${m.description}"
  homepage "${m.homepage}"
  version "${m.version}"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "${archiveUrl(m, TARGETS.darwinArm64)}"
      sha256 "${artifact(m, TARGETS.darwinArm64).sha256}"
    end
    on_intel do
      url "${archiveUrl(m, TARGETS.darwinX64)}"
      sha256 "${artifact(m, TARGETS.darwinX64).sha256}"
    end
  end

  on_linux do
    on_arm do
      url "${archiveUrl(m, TARGETS.linuxArm64)}"
      sha256 "${artifact(m, TARGETS.linuxArm64).sha256}"
    end
    on_intel do
      url "${archiveUrl(m, TARGETS.linuxX64)}"
      sha256 "${artifact(m, TARGETS.linuxX64).sha256}"
    end
  end

  def install
    bin.install Dir["*/runx"].first => "runx"
    bin.install Dir["*/runx-js-worker"].first => "runx-js-worker"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/runx --version")
  end
end
`;
}

function renderScoop(m: Manifest): string {
  return `${JSON.stringify({
    version: m.version,
    description: m.description,
    homepage: m.homepage,
    license: "Apache-2.0",
    architecture: {
      "64bit": {
        url: archiveUrl(m, TARGETS.winX64),
        hash: artifact(m, TARGETS.winX64).sha256,
        extract_dir: archiveStem(m, TARGETS.winX64),
        bin: "runx.exe",
      },
    },
    checkver: {
      github: `https://github.com/${m.repo}`,
      regex: "cli-v([\\d.]+)",
    },
    autoupdate: {
      architecture: {
        "64bit": {
          url: `https://github.com/${m.repo}/releases/download/cli-v$version/runx-$version-${TARGETS.winX64}.zip`,
          extract_dir: `runx-$version-${TARGETS.winX64}`,
        },
      },
    },
  }, null, 2)}\n`;
}

function renderWinget(m: Manifest): readonly { path: string; contents: string }[] {
  const base = "winget";
  const manifestVersion = "1.6.0";
  return [
    {
      path: `${base}/runxhq.runx.yaml`,
      contents: `# yaml-language-server: $schema=https://aka.ms/winget-manifest.version.${manifestVersion}.schema.json
PackageIdentifier: runxhq.runx
PackageVersion: ${m.version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: ${manifestVersion}
`,
    },
    {
      path: `${base}/runxhq.runx.locale.en-US.yaml`,
      contents: `# yaml-language-server: $schema=https://aka.ms/winget-manifest.defaultLocale.${manifestVersion}.schema.json
PackageIdentifier: runxhq.runx
PackageVersion: ${m.version}
PackageLocale: en-US
PackageName: runx
Publisher: runxhq
License: Apache-2.0
ShortDescription: ${m.description}
PackageUrl: ${m.homepage}
ManifestType: defaultLocale
ManifestVersion: ${manifestVersion}
`,
    },
    {
      path: `${base}/runxhq.runx.installer.yaml`,
      contents: `# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.${manifestVersion}.schema.json
PackageIdentifier: runxhq.runx
PackageVersion: ${m.version}
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
  - RelativeFilePath: ${windowsArchivePath(m, TARGETS.winX64, "runx.exe")}
    PortableCommandAlias: runx
  - RelativeFilePath: ${windowsArchivePath(m, TARGETS.winX64, "runx-js-worker.exe")}
Installers:
  - Architecture: x64
    InstallerUrl: ${archiveUrl(m, TARGETS.winX64)}
    InstallerSha256: ${artifact(m, TARGETS.winX64).sha256.toUpperCase()}
ManifestType: installer
ManifestVersion: ${manifestVersion}
`,
    },
  ];
}

function aurExpand(m: Manifest, value: string): string {
  return value.replaceAll("$pkgname", "runx").replaceAll("$pkgver", m.version);
}

function renderPkgbuild(m: Manifest): string {
  if (aurExpand(m, AUR_SOURCE_URL) !== m.source.url || aurExpand(m, AUR_SOURCE_ROOT) !== m.source.root) {
    throw new Error(`the AUR source ${aurExpand(m, AUR_SOURCE_URL)} is not the release source ${m.source.url}`);
  }
  return `# Maintainer: runxhq <dev@runx.ai>
pkgname=runx
pkgver=${m.version}
pkgrel=1
pkgdesc="${m.description}"
arch=('x86_64' 'aarch64')
url="${m.homepage}"
license=('Apache-2.0')
depends=('gcc-libs' 'glibc')
makedepends=('cargo')
conflicts=('runx-bin')
source=("$pkgname-$pkgver.tar.gz::${AUR_SOURCE_URL}")
sha256sums=('${m.source.sha256}')

prepare() {
  cd "${AUR_SOURCE_ROOT}"
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --manifest-path crates/Cargo.toml --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "${AUR_SOURCE_ROOT}"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  cargo build --frozen --release --manifest-path crates/Cargo.toml -p runx-cli -p runx-js-worker
}

package() {
  cd "${AUR_SOURCE_ROOT}"
  install -Dm755 target/release/runx "$pkgdir/usr/bin/runx"
  install -Dm755 target/release/runx-js-worker "$pkgdir/usr/bin/runx-js-worker"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
`;
}

// AUR serves .SRCINFO, not the PKGBUILD, so it is what makepkg --printsrcinfo
// prints for the PKGBUILD above.
function renderAurSrcinfo(m: Manifest): string {
  return `pkgbase = runx
\tpkgdesc = ${m.description}
\tpkgver = ${m.version}
\tpkgrel = 1
\turl = ${m.homepage}
\tarch = x86_64
\tarch = aarch64
\tlicense = Apache-2.0
\tmakedepends = cargo
\tdepends = gcc-libs
\tdepends = glibc
\tconflicts = runx-bin
\tsource = runx-${m.version}.tar.gz::${m.source.url}
\tsha256sums = ${m.source.sha256}

pkgname = runx
`;
}

function renderChocolateyNuspec(m: Manifest): string {
  return `<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd">
  <metadata>
    <id>runx</id>
    <version>${m.version}</version>
    <title>Runx</title>
    <authors>runxhq</authors>
    <owners>runxhq</owners>
    <projectUrl>${xmlEscape(m.homepage)}</projectUrl>
    <packageSourceUrl>https://github.com/${xmlEscape(m.repo)}</packageSourceUrl>
    <licenseUrl>https://github.com/${xmlEscape(m.repo)}/blob/main/LICENSE</licenseUrl>
    <requireLicenseAcceptance>false</requireLicenseAcceptance>
    <description>${xmlEscape(m.description)}</description>
    <tags>runx agent skills cli governed runtime</tags>
  </metadata>
  <files>
    <file src="tools/**" target="tools" />
  </files>
</package>
`;
}

// The worker must sit beside the CLI, so the package shims runx alone and
// keeps both executables in the extracted archive directory.
function renderChocolateyInstall(m: Manifest): string {
  const target = TARGETS.winX64;
  return `$ErrorActionPreference = 'Stop'
$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
Install-ChocolateyZipPackage -PackageName 'runx' -Url '${archiveUrl(m, target)}' -UnzipLocation $toolsDir -Checksum '${artifact(m, target).sha256}' -ChecksumType 'sha256'
$binDir = Join-Path $toolsDir '${archiveStem(m, target)}'
$runxBin = Join-Path $binDir 'runx.exe'
$workerBin = Join-Path $binDir 'runx-js-worker.exe'
if (-not (Test-Path $runxBin) -or -not (Test-Path $workerBin)) {
  throw 'The verified Runx archive must contain runx.exe and runx-js-worker.exe together.'
}
New-Item -ItemType File -Path (Join-Path $binDir 'runx.exe.ignore') -Force | Out-Null
New-Item -ItemType File -Path (Join-Path $binDir 'runx-js-worker.exe.ignore') -Force | Out-Null
Install-BinFile -Name 'runx' -Path $runxBin
`;
}

function renderChocolateyUninstall(): string {
  return `Uninstall-BinFile -Name 'runx'
`;
}

function xmlEscape(value: string): string {
  return value.replace(/&/gu, "&amp;").replace(/</gu, "&lt;").replace(/>/gu, "&gt;").replace(/"/gu, "&quot;");
}

function write(relativePath: string, contents: string): void {
  const filePath = path.join(outDir, relativePath);
  mkdirSync(path.dirname(filePath), { recursive: true });
  writeFileSync(filePath, contents);
  written.push(path.relative(workspaceRoot, filePath).split(path.sep).join("/"));
}

function parseArgs(argv: readonly string[]): { input: string; outDir: string } {
  let input = "";
  let outDir = "dist/channels";
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--input") {
      input = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    if (arg === "--out-dir") {
      outDir = argv[index + 1] ?? "";
      index += 1;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  if (!input) throw new Error("--input requires a path to the release manifest JSON");
  return { input, outDir };
}
