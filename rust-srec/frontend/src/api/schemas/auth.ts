import { z } from 'zod';
import { msg } from '@lingui/core/macro';
import { zodError } from '@/lib/zod-i18n';

// --- Auth Schemas ---
export const LoginRequestSchema = z.object({
  username: z.string().min(1, zodError(msg`Username is required`)),
  password: z.string().min(1, zodError(msg`Password is required`)),
  device_info: z.string().optional(),
});

export const LoginResponseSchema = z.object({
  access_token: z.string(),
  refresh_token: z.string(),
  token_type: z.string(),
  expires_in: z.number(),
  refresh_expires_in: z.number(),
  roles: z.array(z.string()),
  must_change_password: z.boolean(),
});

// The backend's `validate_password_strength` rules, checked here so the form
// reports them in the user's language instead of the backend's English error.
// `\p{Alphabetic}` and `\p{N}` match Rust's `char::is_alphabetic` and
// `char::is_numeric`.
const newPasswordFields = {
  new_password: z
    .string()
    .min(8, zodError(msg`Password must be at least 8 characters`))
    .regex(
      /\p{Alphabetic}/u,
      zodError(msg`Password must contain at least one letter`),
    )
    .regex(/\p{N}/u, zodError(msg`Password must contain at least one number`)),
  confirm_password: z
    .string()
    .min(1, zodError(msg`Confirm password is required`)),
};

const passwordsMatch = (data: {
  new_password: string;
  confirm_password: string;
}) => data.new_password === data.confirm_password;

const passwordsMismatchIssue = {
  ...zodError(msg`Passwords don't match`),
  path: ['confirm_password'],
};

export const ChangePasswordRequestSchema = z
  .object({
    current_password: z
      .string()
      .min(1, zodError(msg`Current password is required`)),
    ...newPasswordFields,
  })
  .refine(passwordsMatch, passwordsMismatchIssue);

/** A first password, set without the current one (desktop app only). */
export const SetPasswordSchema = z
  .object(newPasswordFields)
  .refine(passwordsMatch, passwordsMismatchIssue);
