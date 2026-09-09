import { createStart } from '@tanstack/react-start';
import { linguiMiddleware } from './integrations/lingui/lingui-middleware';
import { sidebarMiddleware } from './integrations/sidebar/sidebar-middleware';
import { themeMiddleware } from './integrations/theme/theme-middleware';

export const startInstance = createStart(() => {
  return {
    requestMiddleware: [linguiMiddleware, themeMiddleware, sidebarMiddleware],
  };
});
