import { createStart } from '@tanstack/react-start';
import { linguiMiddleware } from './integrations/lingui/lingui-middleware';

export const startInstance = createStart(() => {
  return {
    requestMiddleware: [linguiMiddleware],
  };
});
