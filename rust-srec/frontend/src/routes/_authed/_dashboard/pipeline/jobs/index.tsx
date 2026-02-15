import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

// Search params schema for URL persistence
const searchParamsSchema = z.object({
  q: z.string().optional(),
  status: z.string().optional(),
  page: z.number().int().min(0).optional(),
  size: z.number().int().positive().optional(),
});

type SearchParams = z.infer<typeof searchParamsSchema>;

export const Route = createFileRoute('/_authed/_dashboard/pipeline/jobs/')({
  validateSearch: (search): SearchParams => searchParamsSchema.parse(search),
});
