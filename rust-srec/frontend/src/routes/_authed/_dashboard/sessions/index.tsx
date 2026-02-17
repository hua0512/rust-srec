import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

const searchSchema = z.object({
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
  validateSearch: (search) => searchSchema.parse(search),
});
