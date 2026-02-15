import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

const searchSchema = z.object({
  page: z.number().optional().catch(1),
  limit: z.number().optional().catch(50),
  streamer_id: z.string().optional(),
  search: z.string().optional(),
  status: z.enum(['all', 'active', 'completed']).optional().catch('all'),
  timeRange: z
    .enum(['all', 'today', 'yesterday', 'week', 'month', 'custom'])
    .optional()
    .catch('all'),
  from: z.string().optional(),
  to: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/sessions/')({
  validateSearch: searchSchema,
});
