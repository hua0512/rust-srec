import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

// Search params schema for URL persistence — keeps search, the file-type filter,
// and pagination in the URL so they survive leaving this page and reloads.
//
// `format` holds a `MediaFileType` value (`VIDEO`, `DANMU_XML`, ...) to match the
// field of the same name on `MediaOutput`. Upper-casing lets a hand-written
// `?format=video` select a type; it is idempotent, so re-validating the value
// this produces cannot drift. An unrecognised value is passed through to the
// backend, which then matches no rows and yields the empty state.
const searchParamsSchema = z.object({
  q: z.string().optional(),
  format: z
    .string()
    .optional()
    .transform((value) => value?.toUpperCase()),
  page: z.number().int().min(0).optional(),
  size: z.number().int().positive().optional(),
});

type SearchParams = z.infer<typeof searchParamsSchema>;

export const Route = createFileRoute('/_authed/_dashboard/pipeline/outputs')({
  validateSearch: (search): SearchParams => searchParamsSchema.parse(search),
});
