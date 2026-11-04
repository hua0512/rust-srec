import { createCsrfMiddleware } from '@tanstack/react-start';

/** The first, client-facing entry of a header a proxy chain appends to. */
function firstEntry(header: string | null): string | undefined {
  return header?.split(',')[0]?.trim() || undefined;
}

/**
 * `host[:port]` of a `Host`-style header, read with the origin's scheme so that
 * a default port written out (`nas:443` for `https:`) compares equal to none.
 */
function hostOf(header: string | null, protocol: string): string | undefined {
  const first = firstEntry(header);
  if (!first) return undefined;
  try {
    return new URL(`${protocol}//${first}`).host;
  } catch {
    return undefined;
  }
}

function originOf(value: string): string | undefined {
  try {
    const url = new URL(value);
    return url.origin === 'null' ? undefined : url.origin;
  } catch {
    return undefined;
  }
}

/**
 * Exact origins (`scheme://host[:port]`) allowed in addition to the request's
 * own address, from the comma-separated `CSRF_TRUSTED_ORIGINS`. For a proxy
 * that rewrites `Host` without sending `X-Forwarded-Host`. Read once at
 * startup; the client bundle, which never runs the check, has no `process`.
 */
const TRUSTED_ORIGINS: ReadonlySet<string> = (() => {
  const origins = new Set<string>();
  const raw =
    typeof process === 'undefined'
      ? undefined
      : process.env.CSRF_TRUSTED_ORIGINS;
  for (const entry of (raw ?? '').split(',')) {
    const value = entry.trim();
    if (!value) continue;
    const origin = originOf(value);
    if (origin && /^https?:\/\//.test(origin)) {
      origins.add(origin);
    } else {
      console.warn(
        `[CSRF] Ignoring CSRF_TRUSTED_ORIGINS entry "${value}": expected an origin such as https://srec.example.com.`,
      );
    }
  }
  return origins;
})();

/**
 * Whether `origin` (an `Origin` header, or the origin of a `Referer`) names the
 * address the browser sent this request to.
 *
 * Browsers send `Sec-Fetch-Site` only to HTTPS and localhost, so on a plain
 * HTTP install this is the only check. The port is part of the comparison: a
 * page on another port of the same host is same-site, so it receives the
 * SameSite=Lax session cookie, and only the port tells it apart. The SSR server
 * is reached over `http:` behind a TLS terminator, so the scheme comes from
 * `X-Forwarded-Proto` when a proxy sends it (the bundled nginx does). It is
 * compared when the origin uses its scheme's default port, where `http://nas`
 * and `https://nas` would otherwise both match `Host: nas`. The host is taken
 * from `Host`, or from the `X-Forwarded-Host` that a proxy rewriting `Host`
 * leaves behind; a cross-site page can set neither.
 */
export function isRequestHostOrigin(origin: string, request: Request): boolean {
  let url: URL;
  try {
    url = new URL(origin);
  } catch {
    return false;
  }
  if (!url.host || !/^https?:$/.test(url.protocol)) return false;
  const proto = firstEntry(request.headers.get('x-forwarded-proto'));
  if (!url.port && proto && `${proto.toLowerCase()}:` !== url.protocol) {
    return false;
  }
  return [
    request.headers.get('host'),
    request.headers.get('x-forwarded-host'),
  ].some((header) => hostOf(header, url.protocol) === url.host);
}

const FAILURE_MESSAGE =
  'Request refused: it did not come from this site. If the web interface is ' +
  'behind a reverse proxy, forward the original Host header with its port ' +
  '(or set X-Forwarded-Host), or add the address you open it at to ' +
  'CSRF_TRUSTED_ORIGINS.';

const MAX_LOGGED_REFUSALS = 100;
const loggedRefusals = new Set<string>();
let suppressionLogged = false;

/** Logs a refusal once per (origin, host) pair, for the first 100 pairs. */
function logRefusal(headers: Headers): void {
  const referer = headers.get('referer');
  const from =
    headers.get('origin') ??
    (referer
      ? `${originOf(referer) ?? 'an unreadable Referer'} (from Referer)`
      : 'an unknown origin');
  const host = headers.get('host') ?? 'absent';
  const key = `${from} ${host}`;
  if (loggedRefusals.has(key)) return;
  if (loggedRefusals.size >= MAX_LOGGED_REFUSALS) {
    if (!suppressionLogged) {
      suppressionLogged = true;
      console.warn(
        `[CSRF] Further refusals suppressed: only the first ${MAX_LOGGED_REFUSALS} origin/host pairs are logged.`,
      );
    }
    return;
  }
  loggedRefusals.add(key);
  console.warn(
    `[CSRF] Refused a server function call from ${from} ` +
      `(Sec-Fetch-Site: ${headers.get('sec-fetch-site') ?? 'absent'}, Host: ${host}, ` +
      `X-Forwarded-Host: ${headers.get('x-forwarded-host') ?? 'absent'}, ` +
      `X-Forwarded-Proto: ${headers.get('x-forwarded-proto') ?? 'absent'}).`,
  );
}

/**
 * Rejects server function calls that a cross-site page could send, such as a
 * urlencoded or multipart form POST, which the server function handler
 * accepts. Supplying `requestMiddleware` to `createStart` replaces the CSRF
 * middleware TanStack Start installs by default, so it must be listed there.
 * Page requests are not checked.
 */
export const csrfMiddleware = createCsrfMiddleware({
  filter: (ctx) => ctx.handlerType === 'serverFn',
  origin: (origin, { request }) =>
    isRequestHostOrigin(origin, request) ||
    TRUSTED_ORIGINS.has(originOf(origin) ?? ''),
  failureResponse: ({ request }) => {
    logRefusal(request.headers);
    return new Response(FAILURE_MESSAGE, {
      status: 403,
      headers: { 'Content-Type': 'text/plain; charset=utf-8' },
    });
  },
});
