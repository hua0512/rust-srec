import { z } from 'zod';
import { ProxyRouteSchema } from './proxies';

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

const CredentialOwnerSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('platform'), platform_id: ProfileId }).strict(),
  z.object({ type: z.literal('template'), template_id: ProfileId }).strict(),
  z.object({ type: z.literal('streamer'), streamer_id: ProfileId }).strict(),
]);
export type CredentialOwner = z.infer<typeof CredentialOwnerSchema>;

const CredentialMaterialSchema = z
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
  label: z.string(),
  enabled: z.boolean(),
  version: z.number().int(),
  /**
   * The account's own route; `inherit` follows the recording using it. Treated
   * as `inherit` when absent.
   */
  proxy_route: ProxyRouteSchema.optional(),
  /** When an operation last received the account, in epoch milliseconds. */
  last_used_at: z.number().int().nullable().optional(),
});

const CredentialProfileHealthSchema = z.object({
  validity: z.enum(['unknown', 'valid', 'needs_refresh', 'invalid']),
  last_check_at: z.number().int().nullable().optional(),
  last_refresh_at: z.number().int().nullable().optional(),
  /** Consecutive failed refreshes at the current revision. */
  refresh_failure_count: z.number().int().optional(),
  last_failure_at: z.number().int().nullable().optional(),
  reason_code: z
    .enum([
      'login_required',
      'manual_validation',
      'refresh_failed',
      'repair_required',
      'authentication_failed',
    ])
    .nullable()
    .optional(),
});
export type CredentialHealthReason = NonNullable<
  z.infer<typeof CredentialProfileHealthSchema>['reason_code']
>;

/** A scope whose selection lists the profile, with its display name. */
const SelectionReferenceSchema = z.object({
  owner: CredentialOwnerSchema,
  name: z.string(),
  /** The platform the selection chooses accounts on. */
  platform_id: z.string(),
  platform_name: z.string(),
});
export type SelectionReference = z.infer<typeof SelectionReferenceSchema>;

/** A live recording bound to the profile. */
const RecordingReferenceSchema = z.object({
  session_id: z.string(),
  /** Absent once the streamer itself has been deleted. */
  streamer_id: z.string().nullable(),
  streamer_name: z.string(),
});
export type RecordingReference = z.infer<typeof RecordingReferenceSchema>;

/** What keeps a profile in use. */
export const ProfileReferencesSchema = z.object({
  selections: z.array(SelectionReferenceSchema),
  recordings: z.array(RecordingReferenceSchema),
});
export type ProfileReferences = z.infer<typeof ProfileReferencesSchema>;

/** What accounts on a platform accept and support, from its provider. */
export const PlatformCredentialCapabilitiesSchema = z.object({
  refresh_token: z.boolean(),
  access_token: z.boolean(),
  /** An access token alone authenticates, without cookies. */
  token_only: z.boolean(),
  /** A username and password for automatic sign-in. */
  reauth_login: z.boolean(),
  qr_login: z.boolean(),
  check: z.boolean(),
  refresh: z.boolean(),
  /** Accounts are chosen per streamer, as none or one fixed account. */
  per_streamer_selection: z.boolean(),
});
export type PlatformCredentialCapabilities = z.infer<
  typeof PlatformCredentialCapabilitiesSchema
>;

export const CredentialProfileDetailSchema = z.object({
  profile: CredentialProfileSummarySchema,
  /**
   * The sites a Streamlink account is for, sorted. Streamlink streamers on
   * them that choose no account use it.
   */
  sites: z.array(z.string()).optional(),
  health: CredentialProfileHealthSchema.nullable(),
  references: ProfileReferencesSchema,
  capabilities: z.object({
    validate: z.boolean(),
    refresh: z.boolean(),
    qr_login: z.boolean(),
  }),
  /**
   * When the account is next renewed before use, for platforms that renew
   * accounts by age; a past time means the next use renews it.
   */
  next_renewal_at: z.number().int().nullable().optional(),
});
export type CredentialProfileDetail = z.infer<
  typeof CredentialProfileDetailSchema
>;

/** An enabled account that needs the user, from any platform. */
export const CredentialAttentionSchema = z.object({
  profile: CredentialProfileSummarySchema.extend({
    platform_config_id: z.string(),
  }),
  platform_name: z.string(),
  /** `login_required` (a new login is needed) or `refresh_failing`. */
  reason: z.string(),
  health: CredentialProfileHealthSchema,
});
export type CredentialAttention = z.infer<typeof CredentialAttentionSchema>;

/**
 * Why a credential selection has no account to offer, as the backend's
 * `UnavailableReason` reports it. A reason this build does not know, from a
 * newer backend, reads as `unknown` instead of failing the whole response.
 */
export const UnavailableReasonSchema = z
  .enum([
    'login_required',
    'profiles_disabled',
    'bound_profile_unavailable',
    'binding_policy_changed',
    'attempts_exhausted',
    'unknown',
  ])
  .catch('unknown');
export type UnavailableReason = z.infer<typeof UnavailableReasonSchema>;

export const EffectiveCredentialSelectionSchema = z.object({
  configured: CredentialSelectionSchema.nullable(),
  resolved: z
    .object({
      owner: CredentialOwnerSchema,
      selection: CredentialSelectionSchema,
    })
    .nullable(),
  candidates: z.array(CredentialProfileDetailSchema),
  unavailable_reason: UnavailableReasonSchema.nullable(),
  active_binding: z
    .object({ identity: z.object({ profile_id: z.string().optional() }) })
    .nullable()
    .optional(),
  /**
   * For a Streamlink streamer, its site and the accounts that name it.
   * `resolved` comes from that site when the streamer chooses no account and
   * `site.site` is set.
   */
  site: z
    .object({
      /** The host of the streamer's URL, without a leading `www.`. */
      host: z.string(),
      /** The most specific account site that covers the host. */
      site: z.string().nullable(),
      /** Accounts whose sites cover the host, most specific site first. */
      accounts: z.array(z.string()),
    })
    .nullable()
    .optional(),
});

export type EffectiveCredentialSelection = z.infer<
  typeof EffectiveCredentialSelectionSchema
>;

export const CredentialProfileCreateSchema = z
  .object({
    platform_id: ProfileId,
    label: z.string().trim().min(1).max(128),
    enabled: z.boolean(),
    material: CredentialMaterialSchema,
    /** Omitted follows the recording using the account. */
    proxy_route: ProxyRouteSchema.optional(),
    /** The sites a Streamlink account is for, as host names or URLs. */
    sites: z.array(z.string()).optional(),
  })
  .strict();
export const CredentialProfileUpdateSchema = z
  .object({
    id: ProfileId,
    expected_version: z.number().int().positive(),
    label: z.string().trim().min(1).max(128).optional(),
    enabled: z.boolean().optional(),
    replacement: CredentialMaterialSchema.optional(),
    /** Replaces the account's own route; omitted keeps it. */
    proxy_route: ProxyRouteSchema.optional(),
    /** Replaces a Streamlink account's sites; omitted keeps them. */
    sites: z.array(z.string()).optional(),
  })
  .strict();

export const CredentialLoginTargetSchema = z.discriminatedUnion('type', [
  z
    .object({
      type: z.literal('create'),
      platform_id: ProfileId,
      label: z.string().trim().min(1).max(128),
      /** The new account's own route, which the sign-in already uses. */
      proxy_route: ProxyRouteSchema.optional(),
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
});
export const CredentialLoginReceiptSchema = z.object({
  status: z.enum([
    'not_scanned',
    'scanned',
    'expired',
    'completed',
    'conflict',
  ]),
});

/** The platform whose accounts a scope selects from. */
export interface CredentialPlatform {
  id: string;
  name: string;
}
