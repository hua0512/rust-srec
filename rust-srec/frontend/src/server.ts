import {
  createStartHandler,
  defaultStreamHandler,
} from '@tanstack/react-start/server';
import type { Register } from '@tanstack/react-router';
import type { RequestHandler } from '@tanstack/react-start/server';

const baseFetch = createStartHandler(defaultStreamHandler);

// Providing `RequestHandler` from `@tanstack/react-start/server` is required so that the output types don't import it from `@tanstack/start-server-core`
export type ServerEntry = { fetch: RequestHandler<Register> };

export function createServerEntry(entry: ServerEntry): ServerEntry {
  return {
    async fetch(...args) {
      return await entry.fetch(...args);
    },
  };
}

function tryDecodeDevServerFnId(serverFnId: string): {
  file: string;
  export: string;
} | null {
  try {
    const decoded = Buffer.from(serverFnId, 'base64url').toString('utf8');
    const parsed = JSON.parse(decoded) as { file?: string; export?: string };
    if (
      typeof parsed?.file !== 'string' ||
      typeof parsed?.export !== 'string'
    ) {
      return null;
    }
    return { file: parsed.file, export: parsed.export };
  } catch {
    return null;
  }
}

function sleep(ms: number) {
  return new Promise<void>((resolve) => setTimeout(resolve, ms));
}

export default createServerEntry({
  async fetch(
    ...args: Parameters<RequestHandler<Register>>
  ): Promise<Response> {
    const request = args[0];
    const url = new URL(request.url, 'http://localhost');
    const serverFnBase = process.env.TSS_SERVER_FN_BASE ?? '/_serverFn/';
    const isServerFnRequest = url.pathname.startsWith(serverFnBase);

    if (isServerFnRequest) {
      const serverFnId =
        url.pathname.slice(serverFnBase.length).split('/')[0] ?? '';
      console.log(
        `[Server] Inbound Server Function: ${serverFnId} (${url.pathname})`,
      );
    } else {
      console.log(
        `[Server] Inbound Request: ${request.method} ${url.pathname}`,
      );
    }

    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        return await baseFetch(...args);
      } catch (error) {
        const isActionNotFunction =
          error instanceof TypeError &&
          typeof error.message === 'string' &&
          error.message.includes('action is not a function');

        if (isServerFnRequest && isActionNotFunction && attempt === 0) {
          // Dev-time race: server-fn manifest points at an export that isn't ready yet.
          // Retrying is safe here because the handler was never invoked.
          await sleep(25);
          continue;
        }

        if (error instanceof Response) {
          return error;
        }

        if (process.env.NODE_ENV !== 'production') {
          try {
            if (isServerFnRequest) {
              const serverFnId =
                url.pathname.slice(serverFnBase.length).split('/')[0] ?? '';
              const decoded = tryDecodeDevServerFnId(serverFnId);

              if (decoded) {
                console.error('[ServerFn] Failed request', {
                  serverFnId,
                  file: decoded.file,
                  export: decoded.export,
                  url: url.toString(),
                });
              } else {
                console.error('[ServerFn] Failed request', {
                  serverFnId,
                  url: url.toString(),
                });
              }
            }
          } catch {
            // ignore diagnostics failures
          }
        }

        throw error;
      }
    }

    // Unreachable, but keeps TypeScript happy.
    return await baseFetch(...args);
  },
});
