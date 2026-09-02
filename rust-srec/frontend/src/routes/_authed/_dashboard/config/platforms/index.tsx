import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence — keeps the search term in the URL so
// it survives navigation into config/platforms/$platformId and reloads.
const validateSearch = searchParamsValidator({
  q: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/config/platforms/')({
  validateSearch,
});
