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

export const ChangePasswordRequestSchema = z
  .object({
    current_password: z
      .string()
      .min(1, zodError(msg`Current password is required`)),
    new_password: z
      .string()
      .min(8, zodError(msg`Password must be at least 8 characters`)),
    confirm_password: z
      .string()
      .min(1, zodError(msg`Confirm password is required`)),
  })
  .refine((data) => data.new_password === data.confirm_password, {
    ...zodError(msg`Passwords don't match`),
    path: ['confirm_password'],
  });
