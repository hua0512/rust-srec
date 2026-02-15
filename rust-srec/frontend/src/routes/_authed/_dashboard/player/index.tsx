import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

const playerSearchSchema = z.object({
  url: z.string().optional(),
});

export const Route = createFileRoute('/_authed/_dashboard/player/')({
  validateSearch: (search) => playerSearchSchema.parse(search),
});
