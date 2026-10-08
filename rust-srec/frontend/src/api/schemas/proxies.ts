import { z } from 'zod';

/**
 * How a scope connects. Platforms, templates and streamers may inherit from the
 * next scope out; the global route never does. An account that inherits follows
 * the route of the recording using it.
 *
 * The backend rejects fields a route does not have, so each kind is strict.
 */
export const ProxyRouteSchema = z.discriminatedUnion('kind', [
  z.object({ kind: z.literal('inherit') }).strict(),
  z.object({ kind: z.literal('direct') }).strict(),
  z.object({ kind: z.literal('system') }).strict(),
  z.object({ kind: z.literal('proxy'), id: z.string().trim().min(1) }).strict(),
]);
export type ProxyRoute = z.infer<typeof ProxyRouteSchema>;

/**
 * The route that follows the next scope out. Frozen because form defaults and
 * state share it; a change always replaces the whole route.
 */
export const INHERIT_ROUTE: Readonly<ProxyRoute> = Object.freeze({
  kind: 'inherit',
});

/** A saved proxy. Its username is shown; its password never is. */
export const ProxySchema = z.object({
  id: z.string(),
  name: z.string(),
  /** Canonical `scheme://host[:port]`, without a login. */
  url: z.string(),
  scheme: z.string(),
  host: z.string(),
  port: z.number().int().nullable().optional(),
  username: z.string().nullable().optional(),
  has_password: z.boolean(),
  version: z.number().int(),
  created_at: z.number().int(),
  updated_at: z.number().int(),
  /** Routes on global settings, platforms, templates, streamers and accounts naming it. */
  usage_count: z.number().int(),
});
export type SavedProxy = z.infer<typeof ProxySchema>;

const NamedReferenceSchema = z.object({ id: z.string(), name: z.string() });

/** What keeps a saved proxy in use. */
export const ProxyReferencesSchema = z.object({
  global: z.boolean(),
  platforms: z.array(NamedReferenceSchema),
  templates: z.array(
    NamedReferenceSchema.extend({
      /** Deleted, and kept only until the recordings using it finish. */
      being_removed: z.boolean(),
    }),
  ),
  streamers: z.array(NamedReferenceSchema),
  accounts: z.array(
    z.object({
      id: z.string(),
      label: z.string(),
      platform_id: z.string(),
      platform_name: z.string(),
    }),
  ),
});
export type ProxyReferences = z.infer<typeof ProxyReferencesSchema>;

export const ProxyDetailSchema = z.object({
  proxy: ProxySchema,
  references: ProxyReferencesSchema,
});

export const CreateProxySchema = z
  .object({
    name: z.string().trim().min(1).max(128),
    url: z.string().trim().min(1),
    username: z.string().optional(),
    password: z.string().optional(),
  })
  .strict();

export const UpdateProxySchema = z
  .object({
    id: z.string().min(1),
    expected_version: z.number().int(),
    name: z.string().trim().min(1).max(128).optional(),
    url: z.string().trim().min(1).optional(),
    /** A new username; `null` removes the login, omitted keeps it. */
    username: z.string().nullable().optional(),
    /** A new password; omitted keeps the saved one. */
    password: z.string().optional(),
  })
  .strict();

/** The proxy the server's environment configures, read at startup. */
export const SystemProxySchema = z.object({
  detected: z.boolean(),
  /** `scheme://host[:port]`, without any login. */
  url: z.string().nullable().optional(),
  authenticated: z.boolean(),
  no_proxy: z.string().nullable().optional(),
});

export const RouteSourceSchema = z.enum([
  'account',
  'streamer',
  'template',
  'platform',
  'global',
]);
export type RouteSource = z.infer<typeof RouteSourceSchema>;

/** The route a scope's requests take now, and which setting decided it. */
export const EffectiveRouteSchema = z.object({
  kind: z.enum(['direct', 'system', 'proxy']),
  proxy: NamedReferenceSchema.nullable().optional(),
  source: RouteSourceSchema,
});
export type EffectiveRoute = z.infer<typeof EffectiveRouteSchema>;

export const EffectiveRouteQuerySchema = z.object({
  scope_type: z.enum(['global', 'platform', 'template', 'streamer', 'account']),
  scope_id: z.string().min(1).optional(),
  platform_id: z.string().min(1).optional(),
});
export type EffectiveRouteQuery = z.infer<typeof EffectiveRouteQuerySchema>;

/**
 * One check through a saved or unsaved proxy, against a platform's home page
 * or a custom URL. With `proxy_id` and edits, an omitted password keeps the
 * saved one.
 */
export const TestProxySchema = z
  .object({
    proxy_id: z.string().min(1).optional(),
    url: z.string().trim().min(1).optional(),
    username: z.string().nullable().optional(),
    password: z.string().optional(),
    platform: z.string().min(1).optional(),
    target_url: z.string().trim().min(1).optional(),
  })
  .strict()
  .refine(
    (value) =>
      (value.platform === undefined) !== (value.target_url === undefined),
    { message: 'Give exactly one of platform and target_url' },
  );
export type TestProxy = z.infer<typeof TestProxySchema>;

export const ProbeErrorKindSchema = z.enum([
  'proxy_authentication_required',
  'connect_failed',
  'timeout',
  'tls',
  'failed',
]);
export type ProbeErrorKind = z.infer<typeof ProbeErrorKindSchema>;

export const ProbeOutcomeSchema = z.object({
  ok: z.boolean(),
  status: z.number().int().nullable().optional(),
  latency_ms: z.number(),
  error: ProbeErrorKindSchema.nullable().optional(),
});
export type ProbeOutcome = z.infer<typeof ProbeOutcomeSchema>;

/** Platforms the Test check knows a home page for. */
export const PROBE_PLATFORMS = [
  'bilibili',
  'douyin',
  'douyu',
  'huya',
  'twitch',
  'tiktok',
  'soop',
  'twitcasting',
  'acfun',
  'bigo',
  'pandatv',
  'picarto',
  'redbook',
  'weibo',
] as const;
