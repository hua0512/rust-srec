import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend, LONG_BACKEND_TIMEOUT_MS } from '../api';
import { backendPath, PathIdSchema } from '../backend-path';
import {
  EngineConfigSchema,
  CreateEngineRequestSchema,
  UpdateEngineRequestSchema,
  EngineTestResponseSchema,
  FfmpegConfigSchema,
  StreamlinkConfigSchema,
  MesioConfigSchema,
  type EngineConfig,
} from '../../api/schemas';
import { z } from 'zod';

function normalizeConfig(engineType: string, config: Record<string, unknown>) {
  switch (engineType) {
    case 'FFMPEG':
      return parseInput(FfmpegConfigSchema, config);
    case 'STREAMLINK':
      return parseInput(StreamlinkConfigSchema, config);
    case 'MESIO':
      return parseInput(MesioConfigSchema, config);
    default:
      return config;
  }
}

// The backend row: `engine_type` and `config` are stored as text, and `config` may also arrive
// already decoded.
const EngineRowSchema = z.object({
  id: z.string(),
  name: z.string(),
  engine_type: z.string(),
  config: z.unknown(),
});

function parseEngine(json: unknown): EngineConfig {
  const row = EngineRowSchema.parse(json);
  const config =
    typeof row.config === 'string' ? JSON.parse(row.config) : row.config;
  return EngineConfigSchema.parse({ ...row, config });
}

export const listEngines = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/engines');
    const rows = z.array(EngineRowSchema).parse(json);

    return rows.flatMap((row): EngineConfig[] => {
      try {
        return [parseEngine(row)];
      } catch (e) {
        console.error('Failed to parse engine config:', e);
        // Keep the engine listed with default settings when only its config is unreadable. A
        // row of a type this UI does not know is left out; the backend only writes the three.
        const fallback = EngineConfigSchema.safeParse({ ...row, config: {} });
        return fallback.success ? [fallback.data] : [];
      }
    });
  },
);

export const getEngine = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/engines/${id}`);
    return parseEngine(json);
  });

export const createEngine = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof CreateEngineRequestSchema>) => {
    const parsed = parseInput(CreateEngineRequestSchema, data);
    return {
      ...parsed,
      config: normalizeConfig(parsed.engine_type, parsed.config),
    };
  })
  .handler(async ({ data }) => {
    // Backend expects config as JSON value (will be stringified by backend)
    const json = await fetchBackend('/engines', {
      method: 'POST',
      body: JSON.stringify(data),
    });

    return parseEngine(json);
  });

export const updateEngine = createServerFn({ method: 'POST' })
  .validator(
    (d: { id: string; data: z.infer<typeof UpdateEngineRequestSchema> }) => ({
      id: parseInput(PathIdSchema, d.id),
      data: (() => {
        const parsed = parseInput(UpdateEngineRequestSchema, d.data);
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
    const json = await fetchBackend(backendPath`/engines/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    });

    return parseEngine(json);
  });

export const deleteEngine = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/engines/${id}`, { method: 'DELETE' });
  });

export const testEngine = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/engines/${id}/test`, {
      timeoutMs: LONG_BACKEND_TIMEOUT_MS,
    });
    return EngineTestResponseSchema.parse(json);
  });
