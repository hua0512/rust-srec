import { createCsrfMiddleware } from '@tanstack/react-start';

function hostnameOf(host: string | null): string | undefined {
  // A proxy chain lists the client-facing host first.
  const first = host?.split(',')[0]?.trim();
  if (!first) return undefined;
  try {
    return new URL(`http://${first}`).hostname;
  } catch {
    return undefined;
  }
}

/**
 * Whether `origin` (an `Origin` header, or the origin of a `Referer`) names the
 * host the browser sent this request to.
 *
 * Browsers that send `Sec-Fetch-Site` never reach this check; it is the
 * fallback for those that only send `Origin` or `Referer`. The request URL the
 * SSR server sees cannot stand in for the browser's origin: behind a TLS
 * terminator it is `http:`, and the bundled nginx forwards `Host` without the
 * port the browser used. So only the hostname is compared, against `Host` or
 * the `X-Forwarded-Host` that a proxy rewriting `Host` leaves behind. A
 * cross-site page can set neither header, so a foreign hostname still fails.
 */
export function isRequestHostOrigin(origin: string, request: Request): boolean {
  let originHost: string;
  try {
    originHost = new URL(origin).hostname;
  } catch {
    return false;
  }
  return [
    request.headers.get('host'),
    request.headers.get('x-forwarded-host'),
  ].some((host) => hostnameOf(host) === originHost);
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
  origin: (origin, { request }) => isRequestHostOrigin(origin, request),
});
