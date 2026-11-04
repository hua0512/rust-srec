import { createStart } from '@tanstack/react-start';
import { csrfMiddleware } from './integrations/csrf/csrf-middleware';
import { linguiMiddleware } from './integrations/lingui/lingui-middleware';
import { sidebarMiddleware } from './integrations/sidebar/sidebar-middleware';
import { themeMiddleware } from './integrations/theme/theme-middleware';

export const startInstance = createStart(() => {
  return {
    // CSRF first, so a rejected call runs none of the others.
    requestMiddleware: [
      csrfMiddleware,
      linguiMiddleware,
      themeMiddleware,
      sidebarMiddleware,
    ],
  };
});
