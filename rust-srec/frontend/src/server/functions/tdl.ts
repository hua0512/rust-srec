import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import {
  GetTdlStatusRequestSchema,
  SendTdlLoginInputRequestSchema,
  StartTdlLoginRequestSchema,
  StartTdlLoginResponseSchema,
  TdlLoginStatusResponseSchema,
  TdlStatusResponseSchema,
  type SendTdlLoginInputRequest,
  type GetTdlStatusRequest,
  type StartTdlLoginRequest,
} from '../../api/schemas/tdl';

/**
 * TDL login API (backend routes are mounted under `/api/tools/tdl`).
 *
 * Endpoints:
 * - POST /api/tools/tdl/login/start
 * - GET  /api/tools/tdl/login/{session_id}
 * - POST /api/tools/tdl/login/{session_id}/input
 * - POST /api/tools/tdl/login/{session_id}/cancel
 *
 * Notes:
 * - `output` is an array of output *chunks* (not guaranteed to be line-oriented).
 *   Most UIs should join them: `output.join('')`.
 * - To support Telegram 2FA password via API:
 *   - start with `allow_password: true`
 *   - send the password with `{ sensitive: true }` (best-effort output suppression)
 */

export const getTdlStatus = createServerFn({ method: 'POST' })
  .inputValidator((data: GetTdlStatusRequest) =>
    GetTdlStatusRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/tools/tdl/status', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return TdlStatusResponseSchema.parse(json);
  });

export const startTdlLogin = createServerFn({ method: 'POST' })
  .inputValidator((data: StartTdlLoginRequest) =>
    StartTdlLoginRequestSchema.parse(data),
  )
  .handler(async ({ data }) => {
    const json = await fetchBackend('/tools/tdl/login/start', {
      method: 'POST',
      body: JSON.stringify(data),
    });
    return StartTdlLoginResponseSchema.parse(json);
  });

export const getTdlLoginStatus = createServerFn({ method: 'GET' })
  .inputValidator((sessionId: string) => sessionId)
  .handler(async ({ data: sessionId }) => {
    const json = await fetchBackend(`/tools/tdl/login/${sessionId}`);
    return TdlLoginStatusResponseSchema.parse(json);
  });

export const sendTdlLoginInput = createServerFn({ method: 'POST' })
  .inputValidator(
    (d: { sessionId: string; input: SendTdlLoginInputRequest }) => ({
      sessionId: d.sessionId,
      input: SendTdlLoginInputRequestSchema.parse(d.input),
    }),
  )
  .handler(async ({ data: { sessionId, input } }) => {
    await fetchBackend(`/tools/tdl/login/${sessionId}/input`, {
      method: 'POST',
      body: JSON.stringify(input),
    });
  });

export const cancelTdlLogin = createServerFn({ method: 'POST' })
  .inputValidator((sessionId: string) => sessionId)
  .handler(async ({ data: sessionId }) => {
    await fetchBackend(`/tools/tdl/login/${sessionId}/cancel`, {
      method: 'POST',
    });
  });
