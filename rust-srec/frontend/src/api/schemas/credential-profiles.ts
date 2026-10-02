import { z } from 'zod';

const ProfileId = z.string().refine((value) => value.trim().length > 0);

export const CredentialSelectionSchema = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('inherit') }).strict(),
  z.object({ mode: z.literal('none') }).strict(),
  z.object({ mode: z.literal('fixed'), credential_id: ProfileId }).strict(),
  z
    .object({
      mode: z.literal('pool'),
      credential_ids: z
        .array(ProfileId)
        .min(1)
        .refine((ids) => new Set(ids).size === ids.length),
      strategy: z.enum(['round_robin', 'priority']),
      failover: z.boolean(),
      max_attempts: z.number().int().min(1).max(10),
    })
    .strict(),
]);
export type CredentialSelection = z.infer<typeof CredentialSelectionSchema>;

export const CredentialOwnerSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('platform'), platform_id: ProfileId }).strict(),
  z.object({ type: z.literal('template'), template_id: ProfileId }).strict(),
  z.object({ type: z.literal('streamer'), streamer_id: ProfileId }).strict(),
]);
export type CredentialOwner = z.infer<typeof CredentialOwnerSchema>;

export const CredentialMaterialSchema = z
  .object({
    cookies: z
      .string()
      .refine(
        (value) =>
          !Array.from(value).some(
            (character) =>
              character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
          ),
      ),
    refresh_token: z.string().nullable().optional(),
    access_token: z.string().nullable().optional(),
    reauth_config: z
      .object({ username: z.string().min(1), password: z.string().min(1) })
      .strict()
      .nullable()
      .optional(),
  })
  .strict()
  .refine(
    (value) =>
      value.cookies.trim().length > 0 ||
      value.reauth_config != null ||
      Boolean(value.access_token?.trim()),
  );

export const CredentialProfileSummarySchema = z.object({
  id: ProfileId,
  platform_config_id: ProfileId,
  owner: CredentialOwnerSchema,
  label: z.string(),
  enabled: z.boolean(),
  revision: z.number().int(),
  version: z.number().int(),
  has_cookies: z.boolean(),
  has_refresh_token: z.boolean(),
  has_access_token: z.boolean(),
  has_reauth: z.boolean(),
});
export type CredentialProfileSummary = z.infer<
  typeof CredentialProfileSummarySchema
>;

export const CredentialProfileHealthSchema = z.object({
  profile_id: ProfileId,
  revision: z.number().int(),
  validity: z.enum(['unknown', 'valid', 'needs_refresh', 'invalid']),
  last_check_at: z.number().nullable(),
  last_refresh_at: z.number().nullable(),
  cooldown_until: z.number().nullable(),
  reason_code: z.string().nullable(),
});

export const CredentialProfileDetailSchema = z.object({
  profile: CredentialProfileSummarySchema,
  health: CredentialProfileHealthSchema.nullable(),
  references: z.array(z.string()),
  capabilities: z.object({
    validate: z.boolean(),
    refresh: z.boolean(),
    qr_login: z.boolean(),
  }),
});
export type CredentialProfileDetail = z.infer<
  typeof CredentialProfileDetailSchema
>;

export const EffectiveCredentialSelectionSchema = z.object({
  configured: CredentialSelectionSchema.nullable(),
  resolved: z
    .object({
      platform_id: z.string(),
      owner: CredentialOwnerSchema,
      selection: CredentialSelectionSchema,
      generation: z.string(),
    })
    .nullable(),
  candidates: z.array(CredentialProfileDetailSchema),
  unavailable_reason: z.string().nullable(),
  unavailable_retry_at: z.number().nullable().optional(),
  active_binding: z
    .object({
      identity: z.object({
        kind: z.string(),
        profile_id: z.string().optional(),
      }),
      revision: z.number(),
      epoch: z.number(),
    })
    .nullable()
    .optional(),
});
export const CredentialConversionPreviewSchema = z.object({
  owner: CredentialOwnerSchema,
  platform_id: z.string(),
  effective_source: CredentialOwnerSchema.nullable(),
  refresh_source: CredentialOwnerSchema.nullable(),
  copies_platform_login: z.boolean(),
  copies_account_extras: z.boolean(),
  source_choice_required: z.boolean(),
  effective_has_material: z.boolean(),
  refresh_has_material: z.boolean(),
  fingerprint: z.string(),
  existing_profile_id: z.string().nullable(),
});
export const CredentialConversionRequestSchema = z
  .object({
    owner: CredentialOwnerSchema,
    platform_id: z.string().min(1),
    label: z.string().trim().min(1).max(128),
    source: z.enum(['effective', 'refresh_source', 'none']),
    expected_fingerprint: z.string().min(1),
  })
  .strict();

export const CredentialProfileCreateSchema = z
  .object({
    owner: CredentialOwnerSchema,
    platform_id: ProfileId,
    label: z.string().trim().min(1).max(128),
    enabled: z.boolean(),
    material: CredentialMaterialSchema,
  })
  .strict();
export const CredentialProfileUpdateSchema = z
  .object({
    id: ProfileId,
    expected_version: z.number().int().positive(),
    label: z.string().trim().min(1).max(128).optional(),
    enabled: z.boolean().optional(),
    replacement: CredentialMaterialSchema.optional(),
  })
  .strict();

export const CredentialLoginTargetSchema = z.discriminatedUnion('type', [
  z
    .object({
      type: z.literal('create'),
      owner: CredentialOwnerSchema,
      platform_id: ProfileId,
      label: z.string().trim().min(1).max(128),
    })
    .strict(),
  z
    .object({
      type: z.literal('replace'),
      profile_id: ProfileId,
      expected_version: z.number().int().positive(),
    })
    .strict(),
]);
export type CredentialLoginTarget = z.infer<typeof CredentialLoginTargetSchema>;
export const CredentialLoginGeneratedSchema = z.object({
  login_id: z.string(),
  url: z.string(),
  expires_at: z.number(),
});
export const CredentialLoginReceiptSchema = z.object({
  status: z.enum([
    'not_scanned',
    'scanned',
    'expired',
    'completed',
    'conflict',
  ]),
  profile_id: z.string().nullable(),
  version: z.number().nullable(),
});
