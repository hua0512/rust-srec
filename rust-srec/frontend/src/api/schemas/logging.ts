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
