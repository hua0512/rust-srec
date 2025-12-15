import { createServerFn } from '@tanstack/react-start';
import { fetchBackend } from '../api';
import {
  GlobalConfigSchema,
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
const jsonToString = z
  .any()
  .transform((val) =>
    typeof val === 'object' && val !== null ? JSON.stringify(val) : val,
  );

const GlobalConfigWriteSchema = GlobalConfigSchema.extend({
  proxy_config: jsonToString.optional(),
  pipeline: jsonToString.optional(),
});

export const updateGlobalConfig = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof GlobalConfigSchema>) => data)
  .handler(async ({ data }) => {
    const payload = GlobalConfigWriteSchema.parse(data);
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
});

export const createTemplate = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof CreateTemplateRequestSchema>) => data)
  .handler(async ({ data }) => {
    console.log('Creating template:', data);
    const payload = TemplateWriteSchema.parse(data);
    const json = await fetchBackend('/templates', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    console.log('Template created:', json);
    return TemplateSchema.parse(json);
  });

export const updateTemplate = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: z.infer<typeof UpdateTemplateRequestSchema> }) => d,
  )
  .handler(async ({ data: { id, data } }) => {
    console.log('Updating template:', id, data);
    const payload = TemplateWriteSchema.parse(data);
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
