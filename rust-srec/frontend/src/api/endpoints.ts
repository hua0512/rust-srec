import { apiClient, handleApiError } from './client';
import {
    LoginRequestSchema,
    LoginResponseSchema,
    ChangePasswordRequestSchema,
    StreamerSchema,
    CreateStreamerSchema,
    UpdateStreamerSchema,
    FilterSchema,
    CreateFilterRequestSchema,
    UpdateFilterRequestSchema,
    SessionSchema,
    JobSchema,
    GlobalConfigSchema,
    PlatformConfigSchema,
    TemplateSchema,
    CreateTemplateRequestSchema,
    UpdateTemplateRequestSchema,
    HealthSchema,
    PipelineStatsSchema,
    MediaOutputSchema,
    EngineConfigSchema,
    CreateEngineRequestSchema,
    UpdateEngineRequestSchema,
} from './schemas';
import { z } from 'zod';

// --- Auth ---
export const authApi = {
    login: async (data: z.infer<typeof LoginRequestSchema>) => {
        try {
            const json = await apiClient.post('auth/login', { json: data }).json();
            return LoginResponseSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    changePassword: async (data: z.infer<typeof ChangePasswordRequestSchema>) => {
        try {
            await apiClient.post('auth/change-password', { json: data }).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    refresh: async (refreshToken: string) => {
        try {
            const json = await apiClient.post('auth/refresh', { json: { refresh_token: refreshToken } }).json();
            return LoginResponseSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    logout: async (refreshToken: string) => {
        try {
            await apiClient.post('auth/logout', { json: { refresh_token: refreshToken } }).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};

// --- Streamers ---
export const streamerApi = {
    list: async (params?: { page?: number; limit?: number; search?: string; platform?: string; state?: string }) => {
        try {
            const json = await apiClient.get('streamers', { searchParams: params as any }).json();
            const PaginatedStreamerSchema = z.object({
                items: z.array(StreamerSchema),
                total: z.number(),
                limit: z.number(),
                offset: z.number(),
            });
            return PaginatedStreamerSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    get: async (id: string) => {
        try {
            const json = await apiClient.get(`streamers/${id}`).json();
            return StreamerSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    create: async (data: z.infer<typeof CreateStreamerSchema>) => {
        try {
            const json = await apiClient.post('streamers', { json: data }).json();
            return StreamerSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    update: async (id: string, data: z.infer<typeof UpdateStreamerSchema>) => {
        try {
            const json = await apiClient.patch(`streamers/${id}`, { json: data }).json();
            return StreamerSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    delete: async (id: string) => {
        try {
            await apiClient.delete(`streamers/${id}`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    check: async (id: string) => {
        try {
            await apiClient.post(`streamers/${id}/check`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    extractMetadata: async (url: string) => {
        try {
            const json = await apiClient.post('streamers/extract-metadata', { json: { url } }).json();
            const { ExtractMetadataResponseSchema } = await import('./schemas');
            return ExtractMetadataResponseSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    // --- Filters ---
    getFilters: async (streamerId: string) => {
        try {
            const json = await apiClient.get(`streamers/${streamerId}/filters`).json();
            return z.array(FilterSchema).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    createFilter: async (streamerId: string, data: z.infer<typeof CreateFilterRequestSchema>) => {
        try {
            const json = await apiClient.post(`streamers/${streamerId}/filters`, { json: data }).json();
            return FilterSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    updateFilter: async (streamerId: string, filterId: string, data: z.infer<typeof UpdateFilterRequestSchema>) => {
        try {
            const json = await apiClient.patch(`streamers/${streamerId}/filters/${filterId}`, { json: data }).json();
            return FilterSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    deleteFilter: async (streamerId: string, filterId: string) => {
        try {
            await apiClient.delete(`streamers/${streamerId}/filters/${filterId}`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};


// --- Engines ---
export const engineApi = {
    list: async () => {
        try {
            const json = await apiClient.get('engines').json();
            return z.array(EngineConfigSchema).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    get: async (id: string) => {
        try {
            const json = await apiClient.get(`engines/${id}`).json();
            return EngineConfigSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    create: async (data: z.infer<typeof CreateEngineRequestSchema>) => {
        try {
            const json = await apiClient.post('engines', { json: data }).json();
            return EngineConfigSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    update: async (id: string, data: z.infer<typeof UpdateEngineRequestSchema>) => {
        try {
            const json = await apiClient.patch(`engines/${id}`, { json: data }).json();
            return EngineConfigSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    delete: async (id: string) => {
        try {
            await apiClient.delete(`engines/${id}`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    test: async (id: string) => {
        try {
            const json = await apiClient.get(`engines/${id}/test`).json();
            return z.object({
                available: z.boolean(),
                version: z.string().nullable(),
            }).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};

// --- Sessions ---
export const sessionApi = {
    list: async (params?: { streamer_id?: string; active_only?: boolean }) => {
        try {
            const json = await apiClient.get('sessions', { searchParams: params as any }).json();
            return z.array(SessionSchema).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    get: async (id: string) => {
        try {
            const json = await apiClient.get(`sessions/${id}`).json();
            // Assuming the backend returns Session + outputs, or we fetch them separately?
            // For now assume it returns SessionSchema extended with outputs, or just SessionSchema and we fetch outputs separately.
            // Let's assume it returns SessionSchema for now, and extend it if we find out otherwise.
            // Actually requirements say "List associated Media Outputs (files)".
            // Let's assume the session details endpoint includes `outputs: MediaOutput[]` or we fetch `/api/sessions/:id/outputs`?
            // "List associated Media Outputs (files)" - this might be a separate call or included.
            // Let's look at `SessionSchema`. It has `output_count`.
            // I'll assume for detailed view we might need a separate call for outputs if not included.
            // Let's check `api/outputs`? No.
            // Let's look at `endpoints.ts` again.
            // I'll stick to just fetching the session for now.
            return SessionSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};


// --- Pipeline ---
export const pipelineApi = {
    listJobs: async (params?: { status?: string }) => {
        try {
            const json = await apiClient.get('pipeline/jobs', { searchParams: params as any }).json();
            return z.array(JobSchema).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    getStats: async () => {
        try {
            const json = await apiClient.get('pipeline/stats').json();
            return PipelineStatsSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    retryJob: async (id: string) => {
        try {
            await apiClient.post(`pipeline/jobs/${id}/retry`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    cancelJob: async (id: string) => {
        try {
            await apiClient.delete(`pipeline/jobs/${id}`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    create: async (data: { session_id: string }) => {
        try {
            await apiClient.post('pipeline/create', { json: data }).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    listOutputs: async (params?: { session_id?: string }) => {
        try {
            const json = await apiClient.get('pipeline/outputs', { searchParams: params as any }).json();
            return z.array(MediaOutputSchema).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};

// --- Config ---
export const configApi = {
    getGlobal: async () => {
        try {
            const json = await apiClient.get('config/global').json();
            return GlobalConfigSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    updateGlobal: async (data: z.infer<typeof GlobalConfigSchema>) => {
        try {
            await apiClient.patch('config/global', { json: data }).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    listPlatforms: async () => {
        try {
            const json = await apiClient.get('config/platforms').json();
            return z.array(PlatformConfigSchema).parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    updatePlatform: async (id: string, data: Partial<z.infer<typeof PlatformConfigSchema>>) => {
        try {
            const json = await apiClient.patch(`config/platforms/${id}`, { json: data }).json();
            return PlatformConfigSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    listTemplates: async () => {
        try {
            const json = await apiClient.get('templates').json();
            const PaginatedTemplatesSchema = z.object({
                items: z.array(TemplateSchema),
                total: z.number(),
                limit: z.number(),
                offset: z.number(),
            });
            const response = PaginatedTemplatesSchema.parse(json);
            return response.items;
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    createTemplate: async (data: z.infer<typeof CreateTemplateRequestSchema>) => {
        try {
            const json = await apiClient.post('templates', { json: data }).json();
            return TemplateSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    updateTemplate: async (id: string, data: z.infer<typeof UpdateTemplateRequestSchema>) => {
        try {
            const json = await apiClient.patch(`templates/${id}`, { json: data }).json();
            return TemplateSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
    deleteTemplate: async (id: string) => {
        try {
            await apiClient.delete(`templates/${id}`).text();
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};

// --- System ---
export const systemApi = {
    getHealth: async () => {
        try {
            const json = await apiClient.get('health').json();
            return HealthSchema.parse(json);
        } catch (error) {
            throw await handleApiError(error);
        }
    },
};
