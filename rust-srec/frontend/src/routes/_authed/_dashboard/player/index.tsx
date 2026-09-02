import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';
import { searchParamsValidator } from '@/lib/search-params';

const validateSearch = searchParamsValidator({
  url: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/player/')({
  validateSearch,
});
