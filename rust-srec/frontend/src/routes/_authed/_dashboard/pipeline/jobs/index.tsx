import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

// Search params schema for URL persistence
const validateSearch = searchParamsValidator({
  q: z.string().optional(),
  status: z.string().optional(),
  page: z.number().int().min(0).optional(),
  size: z.number().int().positive().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/pipeline/jobs/')({
  validateSearch,
});
