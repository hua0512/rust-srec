import { z } from 'zod';

// --- Logging Configuration Schemas ---

/** Available logging module info */
export const LogModuleInfoSchema = z.object({
  name: z.string(),
  description: z.string(),
});
export type LogModuleInfo = z.infer<typeof LogModuleInfoSchema>;

/** Logging configuration response from API */
export const LoggingConfigResponseSchema = z.object({
  filter: z.string(),
  available_modules: z.array(LogModuleInfoSchema),
});
export type LoggingConfigResponse = z.infer<typeof LoggingConfigResponseSchema>;

/** Request to update logging filter */
export const UpdateLogFilterRequestSchema = z.object({
  filter: z.string(),
});
export type UpdateLogFilterRequest = z.infer<
  typeof UpdateLogFilterRequestSchema
>;

/** Log levels available for configuration */
export const LOG_LEVELS = [
  'trace',
  'debug',
  'info',
  'warn',
  'error',
  'off',
] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

/** Parsed module filter entry */
export interface ModuleFilter {
  module: string;
  level: LogLevel;
}

/** Parse filter directive string into module filters */
export function parseFilterDirective(filter: string): ModuleFilter[] {
  if (!filter) return [];

  return filter
    .split(',')
    .map((part) => {
      const [module, level] = part.trim().split('=');
      return {
        module: module || '',
        level: (level as LogLevel) || 'info',
      };
    })
    .filter((f) => f.module);
}

/** Serialize module filters back to directive string */
export function serializeFilterDirective(filters: ModuleFilter[]): string {
  return filters
    .filter((f) => f.module)
    .map((f) => `${f.module}=${f.level}`)
    .join(',');
}

// --- Log Files Schemas ---

/** Log file info from API */
export const LogFileInfoSchema = z.object({
  date: z.string(),
  filename: z.string(),
  size_bytes: z.number(),
});
export type LogFileInfo = z.infer<typeof LogFileInfoSchema>;

/** Log files list response */
export const LogFilesResponseSchema = z.object({
  items: z.array(LogFileInfoSchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});
export type LogFilesResponse = z.infer<typeof LogFilesResponseSchema>;

/** Archive token response */
export const ArchiveTokenResponseSchema = z.object({
  token: z.string(),
  expires_at: z.string(),
});
export type ArchiveTokenResponse = z.infer<typeof ArchiveTokenResponseSchema>;
