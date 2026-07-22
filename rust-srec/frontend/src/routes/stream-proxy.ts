import { createFileRoute } from '@tanstack/react-router';

export const Route = createFileRoute('/stream-proxy')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!request) {
          return new Response('Internal Server Error', { status: 500 });
        }

        const { handleStreamProxyRequest } =
          await import('@/server/stream-proxy');
        return handleStreamProxyRequest(request);
      },
    },
  },
});
