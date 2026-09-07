import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence — keeps the status/time filters, the
// search term and pagination in the URL so they survive navigation into a
// session detail page and reloads.
const validateSearch = searchParamsValidator({
  page: z.number().optional(),
  limit: z.number().optional(),
  streamer_id: z.string().optional(),
  search: z.string().optional(),
  status: z.enum(['all', 'active', 'completed']).optional(),
  timeRange: z
    .enum(['all', 'today', 'yesterday', 'week', 'month', 'custom'])
    .optional(),
  from: z.string().optional(),
  to: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/sessions/')({
  validateSearch,
});
