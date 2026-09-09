/**
 * The sidebar's expanded/collapsed state lives in a non-httpOnly cookie so the
 * server can render the correct layout and the browser can read and write it
 * without a round trip. Both sides go through this module to stay in step.
 */
export const SIDEBAR_COOKIE_KEY = 'sidebar_state';

export const SIDEBAR_COOKIE_MAX_AGE_SECONDS = 60 * 60 * 24 * 7;

/** Used whenever the visitor has never expressed a preference. */
export const DEFAULT_SIDEBAR_OPEN = true;

/**
 * Reads the state out of a raw `Cookie` header or `document.cookie`, which use
 * the same `name=value; name=value` syntax. Returns `undefined` when the cookie
 * is absent or holds anything other than the two values written below, so
 * callers can tell "no preference" from "collapsed".
 */
export function readSidebarCookie(
  raw: string | undefined,
): boolean | undefined {
  const match = raw?.match(
    new RegExp(`(?:^|; )${SIDEBAR_COOKIE_KEY}=(true|false)`),
  );
  if (!match) return undefined;
  return match[1] === 'true';
}
