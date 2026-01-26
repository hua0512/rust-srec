#!/usr/bin/env node
// Bump the single rust-srec "app version".
//
// Source of truth: root Cargo.toml [workspace.package].version
// This flows into:
// - rust-srec/Cargo.toml (version.workspace = true)
// - rust-srec/src-tauri/Cargo.toml (version.workspace = true)
// - rust-srec/src-tauri/tauri.conf.json (no version; Tauri falls back to Cargo)

import fs from 'node:fs';
import path from 'node:path';

function die(msg) {
  process.stderr.write(`${msg}\n`);
  process.exit(1);
}

function isSemver(v) {
  return /^\d+\.\d+\.\d+$/.test(v);
}

function readText(filePath) {
  return fs.readFileSync(filePath, 'utf8');
}

function writeText(filePath, content) {
  fs.writeFileSync(filePath, content, 'utf8');
}

function setWorkspacePackageVersion(cargoTomlText, version) {
  const header = '[workspace.package]';
  const headerIdx = cargoTomlText.indexOf(header);
  if (headerIdx === -1) {
    die('Missing [workspace.package] in root Cargo.toml');
  }

  const afterHeaderIdx = headerIdx + header.length;
  const rest = cargoTomlText.slice(afterHeaderIdx);
  const nextSectionOffset = rest.search(/^\[/m);
  const sectionEndIdx =
    nextSectionOffset === -1
      ? cargoTomlText.length
      : afterHeaderIdx + nextSectionOffset;

  const section = cargoTomlText.slice(afterHeaderIdx, sectionEndIdx);
  const versionLineRe = /^version\s*=\s*"[^"]+"\s*$/m;

  let newSection;
  if (versionLineRe.test(section)) {
    newSection = section.replace(versionLineRe, `version = "${version}"`);
  } else {
    // Insert version immediately after the section header for readability.
    const prefix = section.startsWith('\r\n')
      ? '\r\n'
      : section.startsWith('\n')
        ? '\n'
        : '\n';
    newSection = `${prefix}version = "${version}"${section}`;
  }

  return (
    cargoTomlText.slice(0, afterHeaderIdx) +
    newSection +
    cargoTomlText.slice(sectionEndIdx)
  );
}

function main() {
  const args = process.argv.slice(2);
  const version = args[0];
  if (!version) {
    die('Usage: node scripts/bump-rust-srec-version.mjs <X.Y.Z>');
  }
  if (!isSemver(version)) {
    die(`Invalid version: ${version} (expected X.Y.Z)`);
  }

  const repoRoot = process.cwd();
  const rootCargoTomlPath = path.join(repoRoot, 'Cargo.toml');
  if (!fs.existsSync(rootCargoTomlPath)) {
    die(`Not found: ${rootCargoTomlPath} (run from repo root)`);
  }

  const before = readText(rootCargoTomlPath);
  const after = setWorkspacePackageVersion(before, version);
  if (after !== before) {
    writeText(rootCargoTomlPath, after);
  }

  process.stdout.write(`Updated Cargo workspace version to ${version}.\n`);
  process.stdout.write(`Next tag: rust-srec-v${version}\n`);
}

main();
