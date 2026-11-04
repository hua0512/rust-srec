import { createCsrfMiddleware } from '@tanstack/react-start';

/** `host[:port]` of a `Host`-style header; a proxy chain lists the client-facing one first. */
function hostOf(header: string | null): string | undefined {
  const first = header?.split(',')[0]?.trim();
  if (!first) return undefined;
  try {
    return new URL(`http://${first}`).host;
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
 * own host, from the comma-separated `CSRF_TRUSTED_ORIGINS`. For a proxy that
 * rewrites `Host` without sending `X-Forwarded-Host`.
 */
function trustedOrigins(): ReadonlySet<string> {
  const origins = new Set<string>();
  for (const entry of (process.env.CSRF_TRUSTED_ORIGINS ?? '').split(',')) {
    const origin = entry.trim() && originOf(entry.trim());
    if (origin) origins.add(origin);
  }
  return origins;
}

/**
 * Whether `origin` (an `Origin` header, or the origin of a `Referer`) names the
 * host and port the browser sent this request to.
 *
 * Browsers send `Sec-Fetch-Site` only to HTTPS and localhost, so on a plain
 * HTTP install this is the only check. The port is part of the comparison: a
 * page on another port of the same host is same-site, so it receives the
 * SameSite=Lax session cookie, and only the port tells it apart. The scheme is
 * not compared, because behind a TLS terminator the SSR server sees `http:`.
 * The host is taken from `Host`, or from the `X-Forwarded-Host` that a proxy
 * rewriting `Host` leaves behind; a cross-site page can set neither.
 */
export function isRequestHostOrigin(origin: string, request: Request): boolean {
  let originHost: string;
  try {
    originHost = new URL(origin).host;
  } catch {
    return false;
  }
  if (!originHost) return false;
  return [
    request.headers.get('host'),
    request.headers.get('x-forwarded-host'),
  ].some((header) => hostOf(header) === originHost);
}

const FAILURE_MESSAGE =
  'Request refused: it did not come from this site. If the web interface is ' +
  'behind a reverse proxy, forward the original Host header (or set ' +
  'X-Forwarded-Host), or add the address you open it at to ' +
  'CSRF_TRUSTED_ORIGINS.';

const loggedRefusals = new Set<string>();

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
    trustedOrigins().has(originOf(origin) ?? ''),
  failureResponse: ({ request }) => {
    const headers = request.headers;
    const key = `${headers.get('origin') ?? '-'} ${headers.get('host') ?? '-'}`;
    // One line per distinct pair, bounded so a flood cannot fill the log.
    if (loggedRefusals.size < 100 && !loggedRefusals.has(key)) {
      loggedRefusals.add(key);
      console.warn(
        `[CSRF] Refused a server function call from ${headers.get('origin') ?? 'an unknown origin'} ` +
          `(Sec-Fetch-Site: ${headers.get('sec-fetch-site') ?? 'absent'}, Host: ${headers.get('host') ?? 'absent'}, ` +
          `X-Forwarded-Host: ${headers.get('x-forwarded-host') ?? 'absent'}).`,
      );
    }
    return new Response(FAILURE_MESSAGE, {
      status: 403,
      headers: { 'Content-Type': 'text/plain; charset=utf-8' },
    });
  },
});
