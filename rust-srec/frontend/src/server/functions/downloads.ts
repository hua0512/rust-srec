import { z } from 'zod';
import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema } from '../backend-path';
import { parseInput } from '../validate';

const SplitResponseSchema = z.object({
  request_id: z.string().regex(/^\d+$/),
  status: z.enum(['pending', 'finalizing']),
});

export const requestLosslessCut = createServerFn({ method: 'POST' })
  .validator((downloadId: string) => parseInput(PathIdSchema, downloadId))
  .handler(async ({ data: downloadId }) => {
    const response = await fetchBackend(
      backendPath`/downloads/${downloadId}/split`,
      {
        method: 'POST',
      },
    );
    return SplitResponseSchema.parse(response);
  });
