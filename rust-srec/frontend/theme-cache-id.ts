import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = path.dirname(fileURLToPath(import.meta.url));

/**
 * Identity for the pre-paint theme css cache, set as the __THEME_CACHE_ID__
 * define in both Vite configs and baked into the blocking script
 * (lib/theme-script.ts). The compiled user-theme css is a pure function of
 * the persisted settings plus these two modules, so a content hash — not a
 * release id — is the right invalidation key: caches survive releases that
 * do not touch theming and invalidate exactly when preset data or the
 * compile logic changes.
 */
export function computeThemeCacheId(): string {
  const hash = createHash('sha256');
  for (const rel of [
    'src/utils/shadcn-ui-theme-presets.ts',
    'src/components/providers/theme-settings-sync.tsx',
  ]) {
    hash.update(readFileSync(path.resolve(dir, rel)));
  }
  return hash.digest('hex').slice(0, 16);
}
