import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
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
  proxy_config: jsonToString.optional(),
  pipeline: jsonToString.optional(),
  session_complete_pipeline: jsonToString.optional(),
  paired_segment_pipeline: jsonToString.optional(),
});

export const updateGlobalConfig = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof GlobalConfigWriteSchema>) => {
    console.log(
      'updateGlobalConfig inputValidator - raw data.pipeline:',
      typeof data.pipeline,
      data.pipeline,
    );
    return data;
  })
  .handler(async ({ data }) => {
    console.log(
      'updateGlobalConfig handler - data.pipeline before parse:',
      typeof data.pipeline,
      data.pipeline,
    );
    const payload = GlobalConfigUpdateSchema.parse(data);
    console.log(
      'updateGlobalConfig handler - payload.pipeline after parse:',
      typeof payload.pipeline,
      payload.pipeline,
    );
    await fetchBackend('/config/global', {
      method: 'PATCH',
      body: JSON.stringify(payload),
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
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/config/platforms/${id}`);
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
  proxy_config: jsonToString.optional(),
  event_hooks: jsonToString.optional(),
  pipeline: jsonToString.optional(),
  session_complete_pipeline: jsonToString.optional(),
  paired_segment_pipeline: jsonToString.optional(),
  platform_specific_config: jsonToString.optional(),
});

export const updatePlatformConfig = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: Partial<z.infer<typeof PlatformConfigSchema>> }) =>
      d,
  )
  .handler(async ({ data: { id, data } }) => {
    console.log('updatePlatformConfig input:', data);
    const payload = PlatformConfigWriteSchema.parse(data);
    console.log('updatePlatformConfig serialized:', payload);
    const json = await fetchBackend(`/config/platforms/${id}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
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
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/templates/${id}`);
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
  danmu_sampling_config: jsonToString.optional(),
  proxy_config: jsonToString.optional(),
  event_hooks: jsonToString.optional(),
  pipeline: jsonToString.optional(),
  session_complete_pipeline: jsonToString.optional(),
  paired_segment_pipeline: jsonToString.optional(),
});

export const createTemplate = createServerFn({ method: 'POST' })
  .inputValidator((data: z.input<typeof CreateTemplateRequestSchema>) =>
    TemplateWriteSchema.parse(data),
  )
  .handler(async ({ data }) => {
    console.log('Creating template:', data);
    const payload = data;
    const json = await fetchBackend('/templates', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    console.log('Template created:', json);
    return TemplateSchema.parse(json);
  });

export const updateTemplate = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: z.input<typeof UpdateTemplateRequestSchema> }) => ({
      id: z.string().parse(d.id),
      data: TemplateWriteSchema.parse(d.data),
    }),
  )
  .handler(async ({ data: { id, data } }) => {
    console.log('Updating template:', id, data);
    const payload = data;
    const json = await fetchBackend(`/templates/${id}`, {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
    console.log('Template updated:', json);
    return TemplateSchema.parse(json);
  });

export const deleteTemplate = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/templates/${id}`, { method: 'DELETE' });
  });

export const cloneTemplate = createServerFn({ method: 'POST' })
  .inputValidator((d: { id: string; new_name: string }) => d)
  .handler(async ({ data }) => {
    const { id, new_name } = data;
    const json = await fetchBackend(`/templates/${id}/clone`, {
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

export const importConfig = createServerFn({ method: 'POST' })
  .inputValidator((data: { config: any; mode: 'merge' | 'replace' }) => data)
  .handler(async ({ data }) => {
    return await fetchBackend('/config/backup/import', {
      method: 'POST',
      body: JSON.stringify(data),
    });
  });
