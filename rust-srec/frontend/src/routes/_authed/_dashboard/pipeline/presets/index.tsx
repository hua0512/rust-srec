import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence — keeps category/search/pagination in
// the URL so they survive back-navigation from presets/$presetId and reloads.
const validateSearch = searchParamsValidator({
  category: z.string().optional(),
  q: z.string().optional(),
  page: z.number().int().min(0).optional(),
  size: z.number().int().positive().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/pipeline/presets/')({
  validateSearch,
});
