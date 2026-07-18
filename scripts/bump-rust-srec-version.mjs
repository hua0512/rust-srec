#!/usr/bin/env node
// Bump the single rust-srec "app version".
//
// Source of truth: root Cargo.toml [workspace.package].version
// This flows into:
// - rust-srec/Cargo.toml (version.workspace = true)
// - rust-srec/src-tauri/Cargo.toml (version.workspace = true)
// - rust-srec/src-tauri/tauri.conf.json (no version; Tauri falls back to Cargo)
//
// After bumping, runs `cargo update --workspace` so Cargo.lock carries the new
// workspace-member versions; release builds use `--locked` and fail otherwise.
//
// Optional docs scaffolding (`--docs`) also:
// - creates versioned release note pages under rust-srec/docs/en|zh/release-notes/
// - updates release-notes index/archive pages
// - adds the new version to the VitePress release-notes sidebar
// - updates rust-srec/docs/release-notes.md guide links
// - updates rust-srec/docs/release-notes-body.md GitHub release template
//
// Pass `--from-latest` to seed the localized release-note pages from the latest
// existing version files before converting them into placeholders.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

function die(msg) {
  process.stderr.write(`${msg}\n`);
  process.exit(1);
}

function isSemver(v) {
  return /^\d+\.\d+\.\d+$/.test(v);
}

function compareSemver(a, b) {
  const aParts = a.split('.').map(Number);
  const bParts = b.split('.').map(Number);

  for (let i = 0; i < 3; i += 1) {
    if (aParts[i] !== bParts[i]) {
      return aParts[i] - bParts[i];
    }
  }

  return 0;
}

function readText(filePath) {
  return fs.readFileSync(filePath, 'utf8');
}

function writeText(filePath, content) {
  fs.writeFileSync(filePath, content, 'utf8');
}

function writeMaybe(filePath, content, dryRun) {
  if (dryRun) {
    process.stdout.write(`[dry-run] Would update ${path.relative(process.cwd(), filePath)}\n`);
    return;
  }

  writeText(filePath, content);
}

function ensureDir(dirPath, dryRun) {
  if (fs.existsSync(dirPath)) {
    return;
  }

  if (dryRun) {
    process.stdout.write(`[dry-run] Would create directory ${path.relative(process.cwd(), dirPath)}\n`);
    return;
  }

  fs.mkdirSync(dirPath, { recursive: true });
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

// Keep Cargo.lock in step with the bumped [workspace.package].version. The
// release workflows build with `--locked`, so a lockfile still pinning the old
// workspace-member versions fails the build. `cargo update --workspace` rewrites
// only the workspace members (rust-srec, rust-srec-desktop) and leaves external
// dependencies at their locked versions.
function syncCargoLock(repoRoot, dryRun) {
  if (dryRun) {
    process.stdout.write('[dry-run] Would run `cargo update --workspace` to sync Cargo.lock\n');
    return;
  }

  try {
    execFileSync('cargo', ['update', '--workspace'], { cwd: repoRoot, stdio: 'inherit' });
  } catch {
    process.stderr.write(
      'WARNING: `cargo update --workspace` failed (cargo missing or offline). ' +
        'Run it manually and commit Cargo.lock before tagging — release builds use --locked.\n'
    );
  }
}

function listVersionedReleaseNoteFiles(dirPath) {
  if (!fs.existsSync(dirPath)) {
    return [];
  }

  return fs
    .readdirSync(dirPath, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name)
    .filter((name) => /^v\d+\.\d+\.\d+\.md$/.test(name))
    .map((name) => ({
      fileName: name,
      version: name.slice(1, -3),
      fullPath: path.join(dirPath, name),
    }))
    .sort((a, b) => compareSemver(b.version, a.version));
}

function findLatestVersionedReleaseNote(dirPath) {
  return listVersionedReleaseNoteFiles(dirPath)[0] ?? null;
}

function scaffoldReleaseNoteContent(templateText, version) {
  return templateText
    .replace(/##\s+`v\d+\.\d+\.\d+`/, `## \`v${version}\``)
    .replace(
      /This update focuses on [^\n]+/,
      'Release summary placeholder. Replace this paragraph with the final high-level summary for the release.'
    )
    .replace(
      /这个版本的重点不在[^\n]+/,
      '发布摘要占位。请将这一段替换为该版本的最终高层总结。'
    )
    .replace(
      /## Highlights[\s\S]*?## Behavior changes \/ review before upgrading/,
      `## Highlights\n\n- Add the most user-visible change here\n- Add the biggest behavior or compatibility change here\n- Add one more important improvement here\n\n## Behavior changes / review before upgrading`
    )
    .replace(
      /## 重点更新[\s\S]*?## 升级前建议重点关注/,
      `## 重点更新\n\n- 在这里补充最值得用户关注的版本变化\n- 在这里补充最大的行为变化或兼容性影响\n- 在这里补充另一个重要改进\n\n## 升级前建议重点关注`
    )
    .replace(
      /## Maintenance[\s\S]*/,
      `## Maintenance\n- Update this section with dependency, CI, or tooling changes\n`
    )
    .replace(
      /## 其他改进[\s\S]*/,
      `## 其他改进\n\n### 录制、处理链与性能\n- 在这里补充录制链或性能方向的改进\n\n### 工程与维护\n- 在这里补充依赖、CI 或工具链更新\n\n## 总结\n\n- 在这里补充适合发布说明收尾的总结\n`
    );
}

function defaultEnglishReleaseNoteTemplate(version) {
  return `# Release Notes

## \`v${version}\`

Release summary placeholder. Replace this paragraph with the final high-level summary for the release.

## Highlights

- Add the most user-visible change here
- Add the biggest behavior or compatibility change here
- Add one more important improvement here

## Behavior changes / review before upgrading

- Add the most important upgrade warning here
- Add any compatibility or migration note here

## Improvements

### Recording, pipeline, and processing
- Add recording or processing improvements here

### Frontend
- Add frontend changes here

### Notifications and visibility
- Add notification or API visibility updates here

### Platform support
- Add platform-specific changes here

## Maintenance
- Update this section with dependency, CI, or tooling changes
`;
}

function defaultChineseReleaseNoteTemplate(version) {
  return `# 更新日志

## \`v${version}\`

发布摘要占位。请将这一段替换为该版本的最终高层总结。

## 重点更新

- 在这里补充最值得用户关注的版本变化
- 在这里补充最大的行为变化或兼容性影响
- 在这里补充另一个重要改进

## 升级前建议重点关注

- 在这里补充最重要的升级提醒
- 在这里补充兼容性或迁移注意事项

## 其他改进

### 录制、处理链与性能
- 在这里补充录制链或性能方向的改进

### 工程与维护
- 在这里补充依赖、CI 或工具链更新

## 总结

- 在这里补充适合发布说明收尾的总结
`;
}

function scaffoldRootReleaseNotesTemplate(templateText, version) {
  const shortBody = `## rust-srec v${version}

Release summary placeholder. Replace this section with the final GitHub Release body.

### Highlights
- Add the most user-visible change here
- Add the biggest behavior or compatibility change here
- Add one more important improvement here

### Review before upgrading
- Add the most important upgrade warning here
- Add any compatibility or migration note here

### Maintenance
- Add dependency, CI, or tooling updates here`;

  return templateText
    .replace(/### rust-srec `v\d+\.\d+\.\d+`/, `### rust-srec \`v${version}\``)
    .replace(/```md\n[\s\S]*?\n```/, `\`\`\`md\n${shortBody}\n\`\`\``)
    .replace(/\/en\/release-notes\/v\d+\.\d+\.\d+/g, `/en/release-notes/v${version}`)
    .replace(/\/zh\/release-notes\/v\d+\.\d+\.\d+/g, `/zh/release-notes/v${version}`)
    .replace(/\.\/en\/release-notes\/v\d+\.\d+\.\d+\.md/g, `./en/release-notes/v${version}.md`)
    .replace(/\.\/zh\/release-notes\/v\d+\.\d+\.\d+\.md/g, `./zh/release-notes/v${version}.md`);
}

function updateReleaseNotesGuide(templateText, version) {
  return templateText
    .replace(/\`v\d+\.\d+\.\d+\`/g, `\`v${version}\``)
    .replace(/\/en\/release-notes\/v\d+\.\d+\.\d+/g, `/en/release-notes/v${version}`)
    .replace(/\/zh\/release-notes\/v\d+\.\d+\.\d+/g, `/zh/release-notes/v${version}`)
    .replace(/\.\/en\/release-notes\/v\d+\.\d+\.\d+\.md/g, `./en/release-notes/v${version}.md`)
    .replace(/\.\/zh\/release-notes\/v\d+\.\d+\.\d+\.md/g, `./zh/release-notes/v${version}.md`);
}

function upsertLatestReleaseIndex(indexText, latestLine, archiveHeading) {
  const latestSectionRe = /(^##\s+Latest release\s*$|^##\s+最新版本\s*$)([\s\S]*?)(^##\s+Archive\s*$|^##\s+历史版本\s*$)/m;
  const match = indexText.match(latestSectionRe);
  if (!match) {
    die('Release notes index is missing Latest/Archive sections');
  }

  const latestHeading = match[1];
  const latestBody = match[2];
  const archiveHeader = match[3];
  const currentLatestLine = latestBody
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line.startsWith('- [`v'));

  let updated = indexText.replace(
    latestSectionRe,
    `${latestHeading}\n\n${latestLine}\n\n${archiveHeader}`
  );

  if (currentLatestLine && currentLatestLine !== latestLine && !updated.includes(currentLatestLine)) {
    updated = updated.replace(
      new RegExp(`(^${archiveHeading}\\s*$)([\\s\\S]*)`, 'm'),
      (_whole, heading, rest) => `${heading}\n\n${currentLatestLine}\n${rest.startsWith('\n') ? rest : `\n${rest}`}`
    );
  }

  return updated;
}

// Insert a `vX.Y.Z` entry into a localized release-notes sidebar section of
// `.vitepress/config.mts`. Section keys are double-quoted (`text: "Release Notes"`
// / `text: "更新日志"`), so match either quote style, and reuse the indentation and
// quote character of the section's existing `items` entries so the inserted lines
// match the surrounding formatting. The new version is placed just before the
// current newest `vX.Y.Z` entry so the list stays in descending order; the whole
// operation is idempotent once `versionLink` is already present.
function ensureVersionInSidebar(configText, sectionLabel, overviewText, overviewLink, version, versionLink) {
  const escapedLabel = sectionLabel.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const sectionRe = new RegExp(
    `(text:\\s*(['"])${escapedLabel}\\2,\\s*items:\\s*\\[)([\\s\\S]*?)(\\n[ \\t]*\\])`
  );
  const match = configText.match(sectionRe);
  if (!match) {
    die(`Could not find sidebar section '${sectionLabel}' in docs config`);
  }

  const prefix = match[1];
  const closing = match[4];
  let itemsBlock = match[3];

  const hasLink = (link) =>
    itemsBlock.includes(`"${link}"`) || itemsBlock.includes(`'${link}'`);

  // Mirror the indentation and quote character of an existing entry.
  const sample = itemsBlock.match(/\n([ \t]*)\{ text: (['"])/);
  const indent = sample ? sample[1] : '                ';
  const quote = sample ? sample[2] : '"';
  const entryLine = (text, link) =>
    `\n${indent}{ text: ${quote}${text}${quote}, link: ${quote}${link}${quote} },`;

  if (!hasLink(overviewLink)) {
    itemsBlock = `${entryLine(overviewText, overviewLink)}${itemsBlock}`;
  }

  if (!hasLink(versionLink)) {
    const newestVersion = itemsBlock.match(/\n[ \t]*\{ text: (['"])v\d+\.\d+\.\d+\1,/);
    if (newestVersion) {
      itemsBlock =
        itemsBlock.slice(0, newestVersion.index) +
        entryLine(`v${version}`, versionLink) +
        itemsBlock.slice(newestVersion.index);
    } else {
      itemsBlock = `${itemsBlock}${entryLine(`v${version}`, versionLink)}`;
    }
  }

  return configText.replace(sectionRe, () => `${prefix}${itemsBlock}${closing}`);
}

function updateDocsForVersion(repoRoot, version, dryRun, fromLatest) {
  const docsRoot = path.join(repoRoot, 'rust-srec', 'docs');
  const enReleaseDir = path.join(docsRoot, 'en', 'release-notes');
  const zhReleaseDir = path.join(docsRoot, 'zh', 'release-notes');
  const enIndexPath = path.join(enReleaseDir, 'index.md');
  const zhIndexPath = path.join(zhReleaseDir, 'index.md');
  const configPath = path.join(docsRoot, '.vitepress', 'config.mts');
  const releaseNotesGuidePath = path.join(docsRoot, 'release-notes.md');
  const releaseNotesBodyPath = path.join(docsRoot, 'release-notes-body.md');
  const newEnPath = path.join(enReleaseDir, `v${version}.md`);
  const newZhPath = path.join(zhReleaseDir, `v${version}.md`);

  ensureDir(enReleaseDir, dryRun);
  ensureDir(zhReleaseDir, dryRun);

  const latestEn = fromLatest ? findLatestVersionedReleaseNote(enReleaseDir) : null;
  const latestZh = fromLatest ? findLatestVersionedReleaseNote(zhReleaseDir) : null;

  if (fromLatest && (!latestEn || !latestZh)) {
    die('Expected existing localized release note templates in rust-srec/docs/en|zh/release-notes');
  }

  if (!fs.existsSync(newEnPath)) {
    const content = fromLatest
      ? scaffoldReleaseNoteContent(readText(latestEn.fullPath), version)
      : defaultEnglishReleaseNoteTemplate(version);
    writeMaybe(newEnPath, content, dryRun);
  }

  if (!fs.existsSync(newZhPath)) {
    const content = fromLatest
      ? scaffoldReleaseNoteContent(readText(latestZh.fullPath), version)
      : defaultChineseReleaseNoteTemplate(version);
    writeMaybe(newZhPath, content, dryRun);
  }

  const enIndex = upsertLatestReleaseIndex(
    readText(enIndexPath),
    `- [\`v${version}\`](./v${version}.md) — draft release notes for the next rust-srec release`,
    '## Archive'
  );
  writeMaybe(enIndexPath, enIndex, dryRun);

  const zhIndex = upsertLatestReleaseIndex(
    readText(zhIndexPath),
    `- [\`v${version}\`](./v${version}.md) — 新版本发布说明草稿，待补充最终摘要`,
    '## 历史版本'
  );
  writeMaybe(zhIndexPath, zhIndex, dryRun);

  let configText = readText(configPath);
  configText = ensureVersionInSidebar(
    configText,
    'Release Notes',
    'Overview',
    '/en/release-notes/',
    version,
    `/en/release-notes/v${version}`
  );
  configText = ensureVersionInSidebar(
    configText,
    '更新日志',
    '概览',
    '/zh/release-notes/',
    version,
    `/zh/release-notes/v${version}`
  );
  writeMaybe(configPath, configText, dryRun);

  let releaseNotesGuide = readText(releaseNotesGuidePath);
  releaseNotesGuide = updateReleaseNotesGuide(releaseNotesGuide, version);
  writeMaybe(releaseNotesGuidePath, releaseNotesGuide, dryRun);

  let releaseNotesBody = readText(releaseNotesBodyPath);
  releaseNotesBody = scaffoldRootReleaseNotesTemplate(releaseNotesBody, version);
  writeMaybe(releaseNotesBodyPath, releaseNotesBody, dryRun);
}

function main() {
  const args = process.argv.slice(2);
  const positional = args.filter((arg) => !arg.startsWith('--'));
  const flags = new Set(args.filter((arg) => arg.startsWith('--')));
  const version = positional[0];
  if (!version) {
    die('Usage: node scripts/bump-rust-srec-version.mjs <X.Y.Z> [--docs] [--dry-run]');
  }
  if (!isSemver(version)) {
    die(`Invalid version: ${version} (expected X.Y.Z)`);
  }

  const allowedFlags = new Set(['--docs', '--dry-run', '--from-latest']);
  for (const flag of flags) {
    if (!allowedFlags.has(flag)) {
      die(`Unknown flag: ${flag}`);
    }
  }

  const updateDocs = flags.has('--docs');
  const dryRun = flags.has('--dry-run');
  const fromLatest = flags.has('--from-latest');

  const repoRoot = process.cwd();
  const rootCargoTomlPath = path.join(repoRoot, 'Cargo.toml');
  if (!fs.existsSync(rootCargoTomlPath)) {
    die(`Not found: ${rootCargoTomlPath} (run from repo root)`);
  }

  const before = readText(rootCargoTomlPath);
  const after = setWorkspacePackageVersion(before, version);
  if (after !== before) {
    writeMaybe(rootCargoTomlPath, after, dryRun);
  }

  syncCargoLock(repoRoot, dryRun);

  if (updateDocs) {
    updateDocsForVersion(repoRoot, version, dryRun, fromLatest);
  }

  process.stdout.write(`${dryRun ? '[dry-run] ' : ''}Updated Cargo workspace version to ${version}.\n`);
  if (updateDocs) {
    process.stdout.write(`${dryRun ? '[dry-run] ' : ''}Updated docs release-note scaffolding for v${version}.\n`);
    if (fromLatest) {
      process.stdout.write(`${dryRun ? '[dry-run] ' : ''}Scaffolded localized release-note pages from the latest existing version templates.\n`);
    } else {
      process.stdout.write(`${dryRun ? '[dry-run] ' : ''}Scaffolded localized release-note pages from the built-in placeholder templates.\n`);
    }
  }
  process.stdout.write(`Next tag: rust-srec-v${version}\n`);
  if (!updateDocs) {
    process.stdout.write('Tip: pass --docs to scaffold versioned release-note docs and update the archive/sidebar.\n');
  }
}

main();
