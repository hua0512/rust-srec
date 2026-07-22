import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

// Search params schema for URL persistence — keeps filters/search/pagination in
// the URL so they survive navigation into a streamer detail/edit page and reloads.
const searchParamsSchema = z.object({
  page: z.number().int().min(1).optional(),
  size: z.number().int().positive().optional(),
  q: z.string().optional(),
  platform: z.string().optional(),
  template: z.string().optional(),
  state: z.string().optional(),
  priority: z.enum(['HIGH', 'NORMAL', 'LOW']).optional(),
  exceptional: z.array(z.string()).optional(),
  sort: z
    .enum([
      'name-asc',
      'name-desc',
      'priority-desc',
      'priority-asc',
      'state-asc',
      'updated-desc',
    ])
    .optional(),
});

type SearchParams = z.infer<typeof searchParamsSchema>;

export const Route = createFileRoute('/_authed/_dashboard/streamers/')({
  validateSearch: (search): SearchParams => searchParamsSchema.parse(search),
});
