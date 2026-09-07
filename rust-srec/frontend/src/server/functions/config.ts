import { createServerFn } from '@/server/createServerFn';
import { parseInput } from '../validate';
import { fetchBackend } from '../api';
import { backendPath, PathIdSchema } from '../backend-path';
import {
  GlobalConfigSchema,
  GlobalConfigWriteSchema,
  PlatformConfigSchema,
  TemplateSchema,
  CreateTemplateRequestSchema,
  UpdateTemplateRequestSchema,
} from '../../api/schemas';
import { z } from 'zod';

// --- Global Config ---
export const getGlobalConfig = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/config/global');
    return GlobalConfigSchema.parse(json);
  },
);

// Helper to stringify complex objects for backend (which expects Option<String>)
const jsonToString = z.any().transform((val) => {
  if (typeof val === 'string') return val;
  if (typeof val === 'object' && val !== null) return JSON.stringify(val);
  return val;
});

// Extend the write schema to handle stringification
const GlobalConfigUpdateSchema = GlobalConfigWriteSchema.extend({
  danmu_statistics: jsonToString.optional(),
  proxy_config: jsonToString.optional(),
  pipeline: jsonToString.optional(),
  session_complete_pipeline: jsonToString.optional(),
  paired_segment_pipeline: jsonToString.optional(),
});

export const updateGlobalConfig = createServerFn({ method: 'POST' })
  .validator((data: z.infer<typeof GlobalConfigWriteSchema>) =>
    parseInput(GlobalConfigUpdateSchema, data),
  )
  .handler(async ({ data }) => {
    await fetchBackend('/config/global', {
      method: 'PATCH',
      body: JSON.stringify(data),
    });
  });

// --- Platforms ---
export const listPlatformConfigs = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/config/platforms');
    return z.array(PlatformConfigSchema).parse(json);
  },
);

export const getPlatformConfig = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/config/platforms/${id}`);
    return PlatformConfigSchema.parse(json);
  });

// Helper to convert empty strings to null
const emptyStringToNull = z
  .union([z.string(), z.null(), z.undefined()])
  .transform((val) => (val === '' ? null : val));

const PlatformConfigWriteSchema = PlatformConfigSchema.partial().extend({
  // Transform empty strings to null for text fields
  cookies: emptyStringToNull,
  output_folder: emptyStringToNull,
  output_filename_template: emptyStringToNull,
  download_engine: emptyStringToNull,
  output_file_format: emptyStringToNull,

  stream_selection_config: jsonToString.optional(),
  download_retry_policy: jsonToString.optional(),
  danmu_statistics: jsonToString.optional(),
  proxy_config: jsonToString.optional(),
  pipeline: jsonToString.optional(),
  session_complete_pipeline: jsonToString.optional(),
  paired_segment_pipeline: jsonToString.optional(),
  platform_specific_config: jsonToString.optional(),
});

export const updatePlatformConfig = createServerFn({ method: 'POST' })
  .validator(
    (d: {
      id: string;
      data: Partial<z.infer<typeof PlatformConfigSchema>>;
    }) => ({
      id: parseInput(PathIdSchema, d.id),
      data: parseInput(PlatformConfigWriteSchema, d.data),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(backendPath`/config/platforms/${id}`, {
      method: 'PUT',
      body: JSON.stringify(data),
    });
    return PlatformConfigSchema.parse(json);
  });

// --- Templates ---
export const listTemplates = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/templates');
    const PaginatedTemplatesSchema = z.object({
      items: z.array(TemplateSchema),
      total: z.number(),
      limit: z.number(),
      offset: z.number(),
    });
    // Endpoints logic extracted .items, duplicating that here
    const response = PaginatedTemplatesSchema.parse(json);
    return response.items;
  },
);

export const getTemplate = createServerFn({ method: 'GET' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(backendPath`/templates/${id}`);
    return TemplateSchema.parse(json);
  });

const TemplateWriteSchema = CreateTemplateRequestSchema.extend({
  // Transform empty strings to null for text fields
  cookies: emptyStringToNull,
  output_folder: emptyStringToNull,
  output_filename_template: emptyStringToNull,
  download_engine: emptyStringToNull,
  output_file_format: emptyStringToNull,

  stream_selection_config: jsonToString.optional(),
  download_retry_policy: jsonToString.optional(),
  danmu_statistics: jsonToString.optional(),
  proxy_config: jsonToString.optional(),
  pipeline: jsonToString.optional(),
  session_complete_pipeline: jsonToString.optional(),
  paired_segment_pipeline: jsonToString.optional(),
});

export const createTemplate = createServerFn({ method: 'POST' })
  .validator((data: z.input<typeof CreateTemplateRequestSchema>) =>
    parseInput(TemplateWriteSchema, data),
  )
  .handler(async ({ data }) => {
    const payload = data;
    const json = await fetchBackend('/templates', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    return TemplateSchema.parse(json);
  });

export const updateTemplate = createServerFn({ method: 'POST' })
  .validator(
    (d: { id: string; data: z.input<typeof UpdateTemplateRequestSchema> }) => ({
      id: parseInput(PathIdSchema, d.id),
      data: parseInput(TemplateWriteSchema, d.data),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    const payload = data;
    const json = await fetchBackend(backendPath`/templates/${id}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
    return TemplateSchema.parse(json);
  });

export const deleteTemplate = createServerFn({ method: 'POST' })
  .validator((id: string) => parseInput(PathIdSchema, id))
  .handler(async ({ data: id }) => {
    await fetchBackend(backendPath`/templates/${id}`, { method: 'DELETE' });
  });

export const cloneTemplate = createServerFn({ method: 'POST' })
  .validator((d: { id: string; new_name: string }) => ({
    id: parseInput(PathIdSchema, d.id),
    new_name: parseInput(z.string().min(1), d.new_name),
  }))
  .handler(async ({ data }) => {
    const { id, new_name } = data;
    const json = await fetchBackend(backendPath`/templates/${id}/clone`, {
      method: 'POST',
      body: JSON.stringify({ new_name }),
    });
    return TemplateSchema.parse(json);
  });

// --- Backup & Restore ---
export const exportConfig = createServerFn({ method: 'GET' }).handler(
  async () => {
    // Returns the raw JSON config object
    return await fetchBackend('/config/backup/export');
  },
);

const ImportConfigSchema = z.object({
  config: z.any(),
  mode: z.enum(['merge', 'replace']),
});

export const importConfig = createServerFn({ method: 'POST' })
  .validator((data: { config: any; mode: 'merge' | 'replace' }) =>
    parseInput(ImportConfigSchema, data),
  )
  .handler(async ({ data }) => {
    return await fetchBackend('/config/backup/import', {
      method: 'POST',
      body: JSON.stringify(data),
    });
  });
