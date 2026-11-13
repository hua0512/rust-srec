import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence — keeps the status/time filters, the
// search term and pagination in the URL so they survive navigation into a
// session detail page and reloads.
const validateSearch = searchParamsValidator({
  page: z.number().int().min(1).optional(),
  limit: z.number().int().positive().optional(),
  streamer_id: z.string().optional(),
  search: z.string().optional(),
  status: z.enum(['all', 'active', 'completed']).optional(),
  timeRange: z
    .enum(['all', 'today', 'yesterday', 'week', 'month', 'custom'])
    .optional(),
  // ISO timestamps, so a hand-edited value is dropped instead of reaching
  // date arithmetic that throws on an invalid date.
  from: z.iso.datetime().optional(),
  to: z.iso.datetime().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/sessions/')({
  validateSearch,
});
