import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
  EngineConfigSchema,
  CreateEngineRequestSchema,
  UpdateEngineRequestSchema,
  EngineTestResponseSchema,
  FfmpegConfigSchema,
  StreamlinkConfigSchema,
  MesioConfigSchema,
} from '../../api/schemas';
import { z } from 'zod';

function normalizeConfig(engineType: string, config: Record<string, unknown>) {
  switch (engineType) {
    case 'FFMPEG':
      return FfmpegConfigSchema.parse(config);
    case 'STREAMLINK':
      return StreamlinkConfigSchema.parse(config);
    case 'MESIO':
      return MesioConfigSchema.parse(config);
    default:
      return config;
  }
}

export const listEngines = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/engines');
    const rawEngines = z.array(z.any()).parse(json);

    // Parse config from JSON string to structured object
    return rawEngines.map((raw: any) => {
      try {
        const config =
          typeof raw.config === 'string' ? JSON.parse(raw.config) : raw.config;

        return EngineConfigSchema.parse({
          ...raw,
          config,
        });
      } catch (e) {
        console.error('Failed to parse engine config:', e);
        // Return with default config if parsing fails
        return {
          ...raw,
          config: {},
        };
      }
    });
  },
);

export const getEngine = createServerFn({ method: 'GET' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/engines/${id}`);
    const raw = json as any;

    // Parse config from JSON string to structured object
    const config =
      typeof raw.config === 'string' ? JSON.parse(raw.config) : raw.config;

    return EngineConfigSchema.parse({
      ...raw,
      config,
    });
  });

export const createEngine = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof CreateEngineRequestSchema>) =>
    (() => {
      const parsed = CreateEngineRequestSchema.parse(data);
      return {
        ...parsed,
        config: normalizeConfig(parsed.engine_type, parsed.config),
      };
    })(),
  )
  .handler(async ({ data }) => {
    // Backend expects config as JSON value (will be stringified by backend)
    const json = await fetchBackend('/engines', {
      method: 'POST',
      body: JSON.stringify(data),
    });

    const raw = json as any;
    const config =
      typeof raw.config === 'string' ? JSON.parse(raw.config) : raw.config;

    return EngineConfigSchema.parse({
      ...raw,
      config,
    });
  });

export const updateEngine = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: z.infer<typeof UpdateEngineRequestSchema> }) => ({
      id: z.string().parse(d.id),
      data: (() => {
        const parsed = UpdateEngineRequestSchema.parse(d.data);
        if (parsed.engine_type && parsed.config) {
          return {
            ...parsed,
            config: normalizeConfig(parsed.engine_type, parsed.config),
          };
        }
        return parsed;
      })(),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    // Backend expects config as JSON value
    const json = await fetchBackend(`/engines/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    });

    const raw = json as any;
    const config =
      typeof raw.config === 'string' ? JSON.parse(raw.config) : raw.config;

    return EngineConfigSchema.parse({
      ...raw,
      config,
    });
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
    return EngineTestResponseSchema.parse(json);
  });
