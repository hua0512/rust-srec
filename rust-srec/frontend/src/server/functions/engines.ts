import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
  EngineConfigSchema,
  CreateEngineRequestSchema,
  UpdateEngineRequestSchema,
} from '../../api/schemas';
import { z } from 'zod';

export const listEngines = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/engines');
    return z.array(EngineConfigSchema).parse(json);
  },
);

export const getEngine = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/engines/${id}`);
    return EngineConfigSchema.parse(json);
  });

export const createEngine = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof CreateEngineRequestSchema>) => data)
  .handler(async ({ data }) => {
    const json = await fetchBackend('/engines', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return EngineConfigSchema.parse(json);
  });

export const updateEngine = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: z.infer<typeof UpdateEngineRequestSchema> }) => d,
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(`/engines/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    });
    return EngineConfigSchema.parse(json);
  });

export const deleteEngine = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/engines/${id}`, { method: 'DELETE' });
  });

export const testEngine = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/engines/${id}/test`);
    return z
      .object({
        available: z.boolean(),
        version: z.string().nullable(),
      })
      .parse(json);
  });
