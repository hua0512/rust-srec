// Mints the frontend's sealed `srec_session` cookie directly, so captures need
// no login. Works against a backend started with AUTH_DISABLED=true, which
// accepts any bearer token.
import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

async function loadIron(frontendDir) {
  const pnpmDir = join(frontendDir, 'node_modules/.pnpm');
  const dir = (await readdir(pnpmDir)).find((d) => d.startsWith('iron-webcrypto@'));
  if (!dir) {
    throw new Error('iron-webcrypto not found; run `pnpm install` in the frontend');
  }
  return import(
    pathToFileURL(join(pnpmDir, dir, 'node_modules/iron-webcrypto/index.js')).href
  );
}

export async function mintSessionCookie({ frontendDir, secret }) {
  const { seal, defaults } = await loadIron(frontendDir);
  const now = Date.now();
  const day = 24 * 3600 * 1000;
  const data = {
    username: 'admin',
    roles: ['admin'],
    mustChangePassword: false,
    token: {
      access_token: 'showcase',
      refresh_token: 'showcase',
      expires_in: now + day,
      refresh_expires_in: now + 7 * day,
    },
  };
  return seal({ data }, secret, { ...defaults, ttl: 7 * day });
}
