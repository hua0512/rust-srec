import { z } from 'zod';

/**
 * Upper bound on a single interpolated path segment. Every backend identifier
 * is a UUID, a slug, or a platform name, so this only bounds the length of a
 * generated request line; it is not a format check.
 */
const MAX_SEGMENT_LENGTH = 512;

/**
 * Validator for an identifier that a server function places in a backend path.
 * `backendPath` enforces the same rule, but rejecting in the validator keeps
 * the failure attributable to the named input rather than to the URL.
 */
export const PathIdSchema = z.string().min(1).max(MAX_SEGMENT_LENGTH);

/** Raised before any request is made when a path parameter is unusable. */
export class InvalidBackendPathError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'InvalidBackendPathError';
  }
}

function encodeSegment(value: string): string {
  if (typeof value !== 'string') {
    throw new InvalidBackendPathError(
      'Backend path parameter must be a string',
    );
  }
  if (value.length === 0) {
    throw new InvalidBackendPathError(
      'Backend path parameter must not be empty',
    );
  }
  if (value.length > MAX_SEGMENT_LENGTH) {
    throw new InvalidBackendPathError(
      `Backend path parameter must be at most ${MAX_SEGMENT_LENGTH} characters`,
    );
  }
  return encodeURIComponent(value);
}

/**
 * Builds a path for `fetchBackend`, percent-encoding every interpolated value
 * as a single path segment:
 *
 * ```ts
 * backendPath`/streamers/${streamerId}/filters/${filterId}`
 * ```
 *
 * In the published deployment the backend is not reachable from the browser, so
 * the server functions are the effective API surface, and `fetchBackend` picks
 * the HTTP method. An identifier carrying `/`, `..`, `?` or `#` would otherwise
 * let a caller aim one function's method at a different endpoint. Interpolated
 * values are therefore always opaque data: literal separators and query strings
 * belong in the template, never in a parameter.
 *
 * @throws InvalidBackendPathError when a parameter is not a string, is empty,
 * or is longer than a single segment may be.
 */
export function backendPath(
  template: TemplateStringsArray,
  ...params: ReadonlyArray<string>
): string {
  let path = template[0];
  for (let index = 0; index < params.length; index += 1) {
    path += encodeSegment(params[index]) + template[index + 1];
  }
  return path;
}

/**
 * Appends `params` to `path` as a query string, omitting the `?` entirely when
 * there is nothing to send.
 */
export function withQuery(path: string, params: URLSearchParams): string {
  const query = params.toString();
  return query ? `${path}?${query}` : path;
}
