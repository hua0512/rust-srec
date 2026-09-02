import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence — keeps filters/search/pagination in
// the URL so they survive navigation into a streamer detail/edit page and reloads.
const validateSearch = searchParamsValidator({
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

export const Route = createFileRoute('/_authed/_dashboard/streamers/')({
  validateSearch,
});
