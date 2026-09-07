import { z } from 'zod';

/** Longest failure summary a toast can show without becoming a wall of text. */
const MAX_ISSUES = 3;

function summarize(error: z.ZodError): string {
  const issues = error.issues;
  if (issues.length === 0) return 'Invalid request';

  const shown = issues.slice(0, MAX_ISSUES).map((issue) => {
    const path = issue.path.join('.');
    return path ? `${path}: ${issue.message}` : issue.message;
  });
  const hidden = issues.length - shown.length;
  return hidden > 0
    ? `${shown.join('; ')} (and ${hidden} more)`
    : shown.join('; ');
}

/**
 * Validates a server-function input and reports a failure as a plain `Error`.
 *
 * A validator runs on the server; whatever it throws is serialized back to the
 * browser and surfaced by the calling mutation, usually as `error.message` in a
 * toast. A raw `ZodError` renders there as its issue array, so failures are
 * flattened to one readable sentence before they leave the server.
 */
export function parseInput<T extends z.ZodType>(
  schema: T,
  value: unknown,
): z.output<T> {
  const result = schema.safeParse(value);
  if (result.success) return result.data;
  throw new Error(summarize(result.error));
}
