import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

// Search params schema for URL persistence — keeps the search term in the URL so
// it survives navigation into config/templates/$templateId and reloads.
const searchParamsSchema = z.object({
  q: z.string().optional(),
});

type SearchParams = z.infer<typeof searchParamsSchema>;

export const Route = createFileRoute('/_authed/_dashboard/config/templates/')({
  validateSearch: (search): SearchParams => searchParamsSchema.parse(search),
});
