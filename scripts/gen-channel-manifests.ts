import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Generates package-manager manifests for a
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
  readonly license: string;
  readonly artifacts: Record<string, Artifact>; // keyed by rust target triple
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
write("macports/Portfile", renderMacPorts(manifest));

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
  license "${m.license}"

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
    license: m.license,
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
License: ${m.license}
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

function renderPkgbuild(m: Manifest): string {
  // AUR's runx-bin belongs to an unrelated project. Keep the package identity
  // unambiguous while installing the released runx binary.
  return `# Maintainer: runxhq <dev@runx.ai>
pkgname=runxhq-bin
pkgver=${m.version}
pkgrel=1
pkgdesc="${m.description}"
arch=('x86_64' 'aarch64')
url="${m.homepage}"
license=('${m.license}')
provides=('runxhq')
conflicts=('runx' 'runx-bin')
source_x86_64=("${archiveUrl(m, TARGETS.linuxX64)}")
source_aarch64=("${archiveUrl(m, TARGETS.linuxArm64)}")
sha256sums_x86_64=('${artifact(m, TARGETS.linuxX64).sha256}')
sha256sums_aarch64=('${artifact(m, TARGETS.linuxArm64).sha256}')

package() {
  case "$CARCH" in
    x86_64) target="${TARGETS.linuxX64}" ;;
    aarch64) target="${TARGETS.linuxArm64}" ;;
    *) echo "unsupported architecture: $CARCH" >&2; return 1 ;;
  esac
  install -Dm755 "runx-\${pkgver}-\${target}/runx" "$pkgdir/usr/bin/runx"
  install -Dm755 "runx-\${pkgver}-\${target}/runx-js-worker" "$pkgdir/usr/bin/runx-js-worker"
}
`;
}

function renderAurSrcinfo(m: Manifest): string {
  const linuxX64 = TARGETS.linuxX64;
  const linuxArm64 = TARGETS.linuxArm64;
  return `pkgbase = runxhq-bin
	pkgdesc = ${m.description}
	pkgver = ${m.version}
	pkgrel = 1
	url = ${m.homepage}
	arch = x86_64
	arch = aarch64
	license = ${m.license}
	provides = runxhq
	conflicts = runx
	conflicts = runx-bin
	source_x86_64 = ${archiveUrl(m, linuxX64)}
	sha256sums_x86_64 = ${artifact(m, linuxX64).sha256}
	source_aarch64 = ${archiveUrl(m, linuxArm64)}
	sha256sums_aarch64 = ${artifact(m, linuxArm64).sha256}

pkgname = runxhq-bin
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

function renderMacPorts(m: Manifest): string {
  const arm = TARGETS.darwinArm64;
  const intel = TARGETS.darwinX64;
  return `PortSystem 1.0

name                runx
version             ${m.version}
categories          sysutils
platforms           darwin
supported_archs     arm64 x86_64
universal_variant   no
license             ${m.license}
maintainers         {runx.ai:dev}
description         {${m.description}}
long_description    {${m.description}}
homepage            ${m.homepage}
master_sites        https://github.com/${m.repo}/releases/download/${m.tag}/

if {$build_arch eq "arm64"} {
    distfiles       ${artifact(m, arm).file}
    checksums       sha256 ${artifact(m, arm).sha256}
    worksrcdir      ${archiveStem(m, arm)}
} elseif {$build_arch eq "x86_64"} {
    distfiles       ${artifact(m, intel).file}
    checksums       sha256 ${artifact(m, intel).sha256}
    worksrcdir      ${archiveStem(m, intel)}
} else {
    known_fail      yes
}

use_configure       no
build               {}

destroot {
    xinstall -m 755 -d $destroot$prefix/bin
    xinstall -m 755 $worksrcpath/runx $destroot$prefix/bin/runx
    xinstall -m 755 $worksrcpath/runx-js-worker $destroot$prefix/bin/runx-js-worker
}
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
