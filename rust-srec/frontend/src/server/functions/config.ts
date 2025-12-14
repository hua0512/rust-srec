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

export const updateGlobalConfig = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof GlobalConfigSchema>) => data)
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
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/config/platforms/${id}`);
    return PlatformConfigSchema.parse(json);
  });

export const updatePlatformConfig = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: Partial<z.infer<typeof PlatformConfigSchema>> }) =>
      d,
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(`/config/platforms/${id}`, {
      method: 'PATCH',
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
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    const json = await fetchBackend(`/templates/${id}`);
    return TemplateSchema.parse(json);
  });

export const createTemplate = createServerFn({ method: 'POST' })
  .inputValidator((data: z.infer<typeof CreateTemplateRequestSchema>) => data)
  .handler(async ({ data }) => {
    const json = await fetchBackend('/templates', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return TemplateSchema.parse(json);
  });

export const updateTemplate = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { id: string; data: z.infer<typeof UpdateTemplateRequestSchema> }) => d,
  )
  .handler(async ({ data: { id, data } }) => {
    const json = await fetchBackend(`/templates/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    });
    return TemplateSchema.parse(json);
  });

export const deleteTemplate = createServerFn({ method: 'POST' })
  .inputValidator((id: string) => id)
  .handler(async ({ data: id }) => {
    await fetchBackend(`/templates/${id}`, { method: 'DELETE' });
  });
