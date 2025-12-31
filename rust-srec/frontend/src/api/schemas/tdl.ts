import { z } from 'zod';

// Matches backend `TdlLoginStatus` (serde externally-tagged enum).
export const TdlLoginStatusSchema = z.union([
  z.literal('running'),
  z.literal('cancelled'),
  z.object({
    exited: z.object({
      code: z.number().nullable().optional(),
    }),
  }),
  z.object({
    failed: z.object({
      message: z.string(),
    }),
  }),
]);

export type TdlLoginStatus = z.infer<typeof TdlLoginStatusSchema>;

export const StartTdlLoginRequestSchema = z.object({
  tdl_path: z.string().optional(),
  working_dir: z.string().optional(),
  env: z.record(z.string(), z.string()).default({}),
  global_args: z.array(z.string()).default([]),
  ttl_secs: z.number().int().positive().optional(),
  allow_password: z.boolean().optional(),
  suppress_output_on_sensitive_input_secs: z.number().int().positive().optional(),
  login_args: z.array(z.string()).default([]),
});

export type StartTdlLoginRequest = z.input<typeof StartTdlLoginRequestSchema>;

export const StartTdlLoginResponseSchema = z.object({
  session_id: z.string(),
  status: TdlLoginStatusSchema,
});

export type StartTdlLoginResponse = z.infer<typeof StartTdlLoginResponseSchema>;

export const TdlLoginStateSchema = z.enum(['logged_in', 'not_logged_in', 'unknown']);
export type TdlLoginState = z.infer<typeof TdlLoginStateSchema>;

export const GetTdlStatusRequestSchema = z.object({
  tdl_path: z.string().optional(),
  working_dir: z.string().optional(),
  env: z.record(z.string(), z.string()).default({}),
  global_args: z.array(z.string()).default([]),
});
export type GetTdlStatusRequest = z.input<typeof GetTdlStatusRequestSchema>;

export const TdlStatusResponseSchema = z.object({
  resolved_tdl_path: z.string(),
  binary_ok: z.boolean(),
  version: z.string().optional(),
  login_state: TdlLoginStateSchema,
  detail: z.string().optional(),
});
export type TdlStatusResponse = z.infer<typeof TdlStatusResponseSchema>;

export const TdlLoginStatusResponseSchema = z.object({
  session_id: z.string(),
  status: TdlLoginStatusSchema,
  output: z.array(z.string()),
});

export type TdlLoginStatusResponse = z.infer<typeof TdlLoginStatusResponseSchema>;

export const SendTdlLoginInputRequestSchema = z.object({
  text: z.string(),
  sensitive: z.boolean().optional(),
});

export type SendTdlLoginInputRequest = z.input<
  typeof SendTdlLoginInputRequestSchema
>;

// --- TDL Processor Config (pipeline preset "tdl") ---
export const TdlLoginTypeSchema = z.enum(['auto', 'qr', 'code', 'desktop']);
export type TdlLoginType = z.infer<typeof TdlLoginTypeSchema>;

export const TdlProcessorConfigSchema = z.object({
  tdl_path: z.string().optional(),
  working_dir: z.string().optional(),
  env: z.record(z.string(), z.string()).default({}),
  // Maps to global `tdl` flags: `--ns <name>` and `--storage <spec>`.
  namespace: z.string().optional(),
  storage: z.string().optional(),
  // Controls the default login flow in the UI.
  login_type: TdlLoginTypeSchema.default('auto'),
  // When enabled, the API allows sending Telegram 2FA password via /api/tools/tdl/login.
  allow_2fa: z.boolean().default(false),
  // Used by the interactive login dialog; passed as `tdl login -d <dir>`.
  telegram_desktop_dir: z.string().optional(),
  // Extra args appended to `tdl login ...` (advanced).
  login_args: z.array(z.string()).default([]),
  args: z.array(z.string()).min(1),
  upload_all: z.boolean().default(false),
  allowed_extensions: z.array(z.string()).optional(),
  excluded_extensions: z.array(z.string()).default([]),
  include_images: z.boolean().default(false),
  include_no_extension: z.boolean().default(false),
  dry_run: z.boolean().default(false),
  max_retries: z.number().int().min(0).default(1),
  continue_on_error: z.boolean().default(false),
});

export type TdlProcessorConfig = z.infer<typeof TdlProcessorConfigSchema>;
