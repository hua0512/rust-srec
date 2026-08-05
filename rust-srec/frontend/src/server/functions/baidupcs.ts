import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import {
  BaiduPcsLoginRequestSchema,
  BaiduPcsLoginResponseSchema,
  BaiduPcsLogoutResponseSchema,
  BaiduPcsStatusResponseSchema,
  BaiduPcsToolRequestSchema,
  type BaiduPcsLoginRequest,
  type BaiduPcsToolRequest,
} from '../../api/schemas/baidupcs';

/**
 * BaiduPCS-Go tool API (backend routes are mounted under
 * `/api/tools/baidupcs`). Login credentials are forwarded once to the
 * backend, which hands them to the CLI; they are never persisted by the
 * app, so callers should clear their inputs after a login attempt.
 */

export const getBaiduPcsStatus = createServerFn({ method: 'POST' })
  .inputValidator((data: BaiduPcsToolRequest) =>
    BaiduPcsToolRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/tools/baidupcs/status', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BaiduPcsStatusResponseSchema.parse(json);
  });

export const baiduPcsLogin = createServerFn({ method: 'POST' })
  .inputValidator((data: BaiduPcsLoginRequest) =>
    BaiduPcsLoginRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/tools/baidupcs/login', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BaiduPcsLoginResponseSchema.parse(json);
  });

export const baiduPcsLogout = createServerFn({ method: 'POST' })
  .inputValidator((data: BaiduPcsToolRequest) =>
    BaiduPcsToolRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/tools/baidupcs/logout', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BaiduPcsLogoutResponseSchema.parse(json);
  });
