import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence — keeps the event-type/priority
// filters, the streamer search, and pagination in the URL so they survive
// navigating away from the events feed and reloads.
const validateSearch = searchParamsValidator({
  type: z.string().optional(),
  priority: z.string().optional(),
  q: z.string().optional(),
  page: z.number().int().min(1).optional(),
});

export const Route = createFileRoute(
  '/_authed/_dashboard/notifications/events',
)({
  validateSearch,
});
