import { z } from 'zod';

/**
 * Build a route's `validateSearch` from an all-optional schema shape, keeping
 * every field that parses and dropping the ones that do not.
 *
 * `schema.parse(search)` throws on the first bad field, and TanStack Router
 * renders that rejection with the route's error component — so a hand-edited,
 * truncated or stale URL such as `?page=abc` replaces the whole page with an
 * error instead of the parameter simply being ignored. Validating field by
 * field keeps the rest of the URL working: `?q=clip&page=abc` still searches
 * for `clip`, it just starts from the first page.
 *
 * Every field must be optional, because that is what a dropped field falls back
 * to. Fields carrying a `.default()` keep it — `safeParse(undefined)` returns
 * the default, which is retained.
 */
export function searchParamsValidator<Shape extends z.ZodRawShape>(
  shape: Shape,
): (search: Record<string, unknown>) => Partial<z.infer<z.ZodObject<Shape>>> {
  const fields = Object.entries(shape) as [string, z.ZodType][];

  return (search) => {
    const validated: Record<string, unknown> = {};
    for (const [key, field] of fields) {
      let result = field.safeParse(search[key]);
      // A rejected value is retried as absent so a field carrying `.default()`
      // falls back to that default rather than being dropped; for a plain
      // optional field the retry yields `undefined` and changes nothing.
      if (!result.success) {
        result = field.safeParse(undefined);
      }
      // `undefined` is omitted rather than stored so the key does not appear in
      // the URL, matching how `useUpdateSearch` clears a filter.
      if (result.success && result.data !== undefined) {
        validated[key] = result.data;
      }
    }
    return validated as Partial<z.infer<z.ZodObject<Shape>>>;
  };
}
