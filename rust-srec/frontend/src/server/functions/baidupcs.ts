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
 * CLI; the backend retains a separate copy only when the request opts into
 * automatic re-login. Callers should clear their inputs after an attempt.
 */

export const getBaiduPcsStatus = createServerFn({ method: 'POST' })
  .validator((data: BaiduPcsToolRequest) =>
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
  .validator((data: BaiduPcsLoginRequest) =>
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
  .validator((data: BaiduPcsToolRequest) =>
    BaiduPcsToolRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/tools/baidupcs/logout', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return BaiduPcsLogoutResponseSchema.parse(json);
  });
