import { z } from 'zod';

// Mirrors the DTOs in `src/api/routes/baidupcs.rs` (mounted at
// `/api/tools/baidupcs`). Credentials only ever travel client -> backend;
// no response schema carries them back.

export const BaiduPcsToolRequestSchema = z.object({
  binary_path: z.string().optional(),
  config_dir: z.string().optional(),
});
export type BaiduPcsToolRequest = z.infer<typeof BaiduPcsToolRequestSchema>;

export const BaiduPcsStatusResponseSchema = z.object({
  resolved_binary_path: z.string(),
  binary_ok: z.boolean(),
  version: z.string().nullable().optional(),
  logged_in: z.boolean(),
  uid: z.number().nullable().optional(),
  username: z.string().nullable().optional(),
  quota_used_bytes: z.number().nullable().optional(),
  quota_total_bytes: z.number().nullable().optional(),
  has_stored_credentials: z.boolean().default(false),
  detail: z.string().nullable().optional(),
});
export type BaiduPcsStatusResponse = z.infer<
  typeof BaiduPcsStatusResponseSchema
>;

export const BaiduPcsLoginRequestSchema = z.object({
  bduss: z.string().optional(),
  stoken: z.string().optional(),
  cookies: z.string().optional(),
  binary_path: z.string().optional(),
  config_dir: z.string().optional(),
  remember: z.boolean().optional(),
});
export type BaiduPcsLoginRequest = z.infer<typeof BaiduPcsLoginRequestSchema>;

export const BaiduPcsLoginResponseSchema = z.object({
  success: z.boolean(),
  uid: z.number().nullable().optional(),
  username: z.string().nullable().optional(),
  credentials_stored: z.boolean().default(false),
  message: z.string(),
});
export type BaiduPcsLoginResponse = z.infer<typeof BaiduPcsLoginResponseSchema>;

export const BaiduPcsLogoutResponseSchema = z.object({
  success: z.boolean(),
  message: z.string(),
});
export type BaiduPcsLogoutResponse = z.infer<
  typeof BaiduPcsLogoutResponseSchema
>;
