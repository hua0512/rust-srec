import { z } from 'zod';

export const CredentialSourceResponseSchema = z.object({
  platform: z.string(),
  scope_type: z.string(),
  scope_id: z.string(),
  scope_name: z.string(),
  has_refresh_token: z.boolean(),
  cookie_length: z.number(),
});

export const CredentialRefreshResponseSchema = z.object({
  refreshed: z.boolean(),
  requires_relogin: z.boolean(),
  source: CredentialSourceResponseSchema.nullable().optional(),
});

// QR Login schemas
export const QrGenerateResponseSchema = z.object({
  url: z.string(),
  auth_code: z.string(),
});

export const QrPollResponseSchema = z.object({
  status: z.enum(['not_scanned', 'scanned', 'expired', 'success']),
  success: z.boolean(),
  message: z.string(),
});

export type CredentialSourceResponse = z.infer<
  typeof CredentialSourceResponseSchema
>;
export type CredentialRefreshResponse = z.infer<
  typeof CredentialRefreshResponseSchema
>;
export type QrGenerateResponse = z.infer<typeof QrGenerateResponseSchema>;
export type QrPollResponse = z.infer<typeof QrPollResponseSchema>;
