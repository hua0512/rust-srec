import { createSerializationAdapter } from '@tanstack/react-router';
import { createStart } from '@tanstack/react-start';
import { BackendApiError } from './lib/api-error';
import { csrfMiddleware } from './integrations/csrf/csrf-middleware';
import { linguiMiddleware } from './integrations/lingui/lingui-middleware';
import { sidebarMiddleware } from './integrations/sidebar/sidebar-middleware';
import { themeMiddleware } from './integrations/theme/theme-middleware';

/**
 * Carries a `BackendApiError` thrown by a server function to the browser
 * whole. Start's default error serialization keeps only the message, which
 * would drop the status and the body the checks in `lib/api-error.ts` read.
 */
const backendApiErrorAdapter = createSerializationAdapter({
  key: 'srec/BackendApiError',
  test: (value): value is BackendApiError => value instanceof BackendApiError,
  // The body is parsed JSON or text, so it round-trips as a JSON string.
  toSerializable: (error) => ({
    status: error.status,
    statusText: error.statusText,
    body: JSON.stringify(error.body ?? null),
  }),
  fromSerializable: ({ status, statusText, body }) =>
    new BackendApiError(status, statusText, JSON.parse(body) as unknown),
});

export const startInstance = createStart(() => {
  return {
    serializationAdapters: [backendApiErrorAdapter],
    // CSRF first, so a rejected call runs none of the others.
    requestMiddleware: [
      csrfMiddleware,
      linguiMiddleware,
      themeMiddleware,
      sidebarMiddleware,
    ],
  };
});
