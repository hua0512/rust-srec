import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

// Search params schema for URL persistence — keeps the event-type/priority
// filters, the streamer search, and pagination in the URL so they survive
// navigating away from the events feed and reloads.
const searchParamsSchema = z.object({
  type: z.string().optional(),
  priority: z.string().optional(),
  q: z.string().optional(),
  page: z.number().int().min(1).optional(),
});

type SearchParams = z.infer<typeof searchParamsSchema>;

export const Route = createFileRoute(
  '/_authed/_dashboard/notifications/events',
)({
  validateSearch: (search): SearchParams => searchParamsSchema.parse(search),
});
