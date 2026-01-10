import { createFileRoute } from '@tanstack/react-router';

export const Route = createFileRoute('/stream-proxy')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!request) {
          throw new Error('Internal Server Error: Request not found');
        }
        const url = new URL(request.url);
        const targetUrl = url.searchParams.get('url');
        const headersParam = url.searchParams.get('headers');

        console.log('Proxying to: ', targetUrl);

        if (!targetUrl) {
          return new Response('Missing url parameter', { status: 400 });
        }

        // Parse custom headers from query param
        let customHeaders: Record<string, string> = {};
        if (headersParam) {
          try {
            customHeaders = JSON.parse(headersParam);
            console.log(customHeaders);
          } catch {
            return new Response('Invalid headers JSON', { status: 400 });
          }
        }

        try {
          // If the client disconnects (e.g. player unmount), propagate abort to upstream.
          // This prevents undici warnings like "Response body is not closed" when the
          // upstream Response is GC'd while still streaming.
          const upstreamAbortController = new AbortController();
          if (request.signal.aborted) {
            upstreamAbortController.abort(request.signal.reason);
          } else {
            request.signal.addEventListener(
              'abort',
              () => upstreamAbortController.abort(request.signal.reason),
              { once: true },
            );
          }

          // Merge headers manually to avoid duplicates (case-insensitive)
          const mergedHeaders = new Headers();
          // Set allowed defaults
          mergedHeaders.set(
            'User-Agent',
            'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36',
          );

          // Set custom headers (overriding defaults)
          if (headersParam) {
            for (const [key, value] of Object.entries(customHeaders)) {
              // Skip dangerous/forbidden headers
              if (['host', 'connection'].includes(key.toLowerCase())) continue;
              mergedHeaders.set(key, value);
            }
          }

          // // Force Origin/Referer if not present or for Douyin specifically
          // if (!mergedHeaders.has('Referer') || targetUrl.includes('douyin')) {
          //     mergedHeaders.set('Referer', 'https://live.douyin.com/');
          // }
          // if (!mergedHeaders.has('Origin') || targetUrl.includes('douyin')) {
          //     mergedHeaders.set('Origin', 'https://live.douyin.com');
          // }

          // Forward range header
          const rangeHeader = request.headers.get('Range');
          if (rangeHeader) {
            mergedHeaders.set('Range', rangeHeader);
          }

          console.log(
            'Upstream Headers:',
            Object.fromEntries(mergedHeaders.entries()),
          );

          // Forward request to target URL
          const response = await fetch(targetUrl, {
            headers: mergedHeaders,
            redirect: 'follow', // Ensure we follow redirects
            signal: upstreamAbortController.signal,
          });

          console.log('Upstream Response Status:', response.status);

          // Build response headers (copy from upstream)
          const responseHeaders = new Headers();

          // console.log('Upstream Response Headers:', Object.fromEntries(response.headers.entries()));

          // Headers to forward to client
          const allowedResponseHeaders = [
            'content-type',
            // 'content-length', // content-length matches the upstream size (which may be gzipped). If we decopress or stream, this might be wrong. Safer to omit.
            'content-range',
            'accept-ranges',
            'cache-control',
            'etag',
            'last-modified',
            'date',
          ];

          for (const key of allowedResponseHeaders) {
            const value = response.headers.get(key);
            if (value) responseHeaders.set(key, value);
          }

          // Explicitly remove content-encoding to prevent browser double-decoding if node-fetch handled it
          responseHeaders.delete('content-encoding');
          responseHeaders.delete('transfer-encoding');

          // Enable CORS for the player
          responseHeaders.set('Access-Control-Allow-Origin', '*');
          responseHeaders.set(
            'Access-Control-Allow-Methods',
            'GET, HEAD, OPTIONS',
          );
          responseHeaders.set('Access-Control-Allow-Headers', 'Range');
          responseHeaders.set(
            'Access-Control-Expose-Headers',
            'Content-Length, Content-Range, Accept-Ranges',
          );

          // Stream the upstream body through a wrapper ReadableStream so the upstream
          // Response body is considered consumed/closed by the runtime (and we can
          // cancel it when the client aborts).
          const upstreamBody = response.body;
          if (!upstreamBody) {
            return new Response(null, {
              status: response.status,
              statusText: response.statusText,
              headers: responseHeaders,
            });
          }

          const upstreamReader = upstreamBody.getReader();
          const proxyBody = new ReadableStream<Uint8Array>({
            async pull(controller) {
              try {
                const { done, value } = await upstreamReader.read();
                if (done) {
                  controller.close();
                  return;
                }
                if (value) controller.enqueue(value);
              } catch (err) {
                // If the client aborted, treat as a clean close.
                if (upstreamAbortController.signal.aborted) {
                  controller.close();
                  return;
                }
                controller.error(err);
              }
            },
            cancel(reason) {
              upstreamAbortController.abort(reason);
              upstreamReader.cancel(reason).catch(() => undefined);
            },
          });

          return new Response(proxyBody, {
            status: response.status,
            statusText: response.statusText,
            headers: responseHeaders,
          });
        } catch (error) {
          console.error('[Proxy] Error:', error);
          return new Response(
            `Proxy error: ${error instanceof Error ? error.message : 'Unknown error'}`,
            { status: 502 },
          );
        }
      },
    },
  },
});
