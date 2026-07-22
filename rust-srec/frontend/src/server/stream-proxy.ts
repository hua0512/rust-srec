import { lookup } from 'node:dns/promises';
import { BlockList, isIP } from 'node:net';
import { ensureValidToken } from './tokenRefresh';

const MAX_REDIRECTS = 5;
const MAX_MANIFEST_BYTES = 8 * 1024 * 1024;
const HLS_MAGIC_SCAN_BYTES = 10;
const USER_AGENT =
  'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36';
const HOP_BY_HOP_HEADERS = new Set([
  'connection',
  'content-length',
  'host',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
]);
const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308]);

const blockedAddresses = new BlockList();
blockedAddresses.addSubnet('0.0.0.0', 8, 'ipv4');
blockedAddresses.addSubnet('10.0.0.0', 8, 'ipv4');
blockedAddresses.addSubnet('100.64.0.0', 10, 'ipv4');
blockedAddresses.addSubnet('127.0.0.0', 8, 'ipv4');
blockedAddresses.addSubnet('169.254.0.0', 16, 'ipv4');
blockedAddresses.addSubnet('172.16.0.0', 12, 'ipv4');
blockedAddresses.addSubnet('192.0.0.0', 24, 'ipv4');
blockedAddresses.addSubnet('192.0.2.0', 24, 'ipv4');
blockedAddresses.addSubnet('192.168.0.0', 16, 'ipv4');
blockedAddresses.addSubnet('198.18.0.0', 15, 'ipv4');
blockedAddresses.addSubnet('198.51.100.0', 24, 'ipv4');
blockedAddresses.addSubnet('203.0.113.0', 24, 'ipv4');
blockedAddresses.addSubnet('224.0.0.0', 4, 'ipv4');
blockedAddresses.addSubnet('240.0.0.0', 4, 'ipv4');
blockedAddresses.addSubnet('2001:db8::', 32, 'ipv6');

export type AddressResolver = (
  hostname: string,
) => Promise<ReadonlyArray<{ address: string; family: number }>>;

class ProxyRequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

function isPublicAddress(address: string): boolean {
  const family = isIP(address);
  if (family === 4) {
    return !blockedAddresses.check(address, 'ipv4');
  }
  if (family !== 6) return false;

  const firstSegment = Number.parseInt(address.split(':', 1)[0] || '0', 16);
  return (
    (firstSegment & 0xe000) === 0x2000 &&
    !blockedAddresses.check(address, 'ipv6')
  );
}

const resolveAddresses: AddressResolver = async (hostname) => {
  return lookup(hostname, { all: true, verbatim: true });
};

export async function validateProxyTarget(
  input: string | URL,
  resolver: AddressResolver = resolveAddresses,
): Promise<URL> {
  let target: URL;
  try {
    target = input instanceof URL ? new URL(input) : new URL(input);
  } catch {
    throw new ProxyRequestError(400, 'Invalid url parameter');
  }

  if (target.protocol !== 'http:' && target.protocol !== 'https:') {
    throw new ProxyRequestError(400, 'Only http/https URLs are allowed');
  }
  if (target.username || target.password) {
    throw new ProxyRequestError(400, 'URL credentials are not allowed');
  }

  const hostname = target.hostname.replace(/^\[|\]$/g, '');
  if (!hostname || hostname.toLowerCase() === 'localhost') {
    throw new ProxyRequestError(400, 'Target host is not allowed');
  }

  const literalFamily = isIP(hostname);
  if (literalFamily !== 0) {
    if (!isPublicAddress(hostname)) {
      throw new ProxyRequestError(400, 'Target host is not allowed');
    }
    return target;
  }

  let addresses: ReadonlyArray<{ address: string; family: number }>;
  try {
    addresses = await resolver(hostname);
  } catch {
    throw new ProxyRequestError(400, 'Target hostname could not be resolved');
  }
  if (
    addresses.length === 0 ||
    addresses.some(({ address }) => !isPublicAddress(address))
  ) {
    throw new ProxyRequestError(400, 'Target host is not allowed');
  }

  return target;
}

function parseCustomHeaders(raw: string | null): Record<string, string> {
  if (!raw) return {};

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new ProxyRequestError(400, 'Invalid headers JSON');
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new ProxyRequestError(400, 'Invalid headers JSON');
  }

  const entries = Object.entries(parsed);
  if (entries.length > 64) {
    throw new ProxyRequestError(400, 'Too many custom headers');
  }

  const headers: Record<string, string> = {};
  for (const [name, value] of entries) {
    if (typeof value !== 'string') {
      throw new ProxyRequestError(400, 'Header values must be strings');
    }
    if (name.length > 256 || value.length > 16_384) {
      throw new ProxyRequestError(400, 'Custom header is too large');
    }
    headers[name] = value;
  }
  return headers;
}

function buildUpstreamHeaders(
  customHeaders: Record<string, string>,
  request: Request,
): Headers {
  const headers = new Headers({ 'User-Agent': USER_AGENT });
  for (const [name, value] of Object.entries(customHeaders)) {
    if (HOP_BY_HOP_HEADERS.has(name.toLowerCase())) continue;
    headers.set(name, value);
  }

  const range = request.headers.get('Range');
  if (range) headers.set('Range', range);
  return headers;
}

async function fetchWithValidatedRedirects(
  initialTarget: URL,
  headers: Headers,
  signal: AbortSignal,
): Promise<{ response: Response; finalUrl: URL }> {
  let target = initialTarget;

  for (let redirectCount = 0; redirectCount <= MAX_REDIRECTS; redirectCount++) {
    target = await validateProxyTarget(target);
    const response = await fetch(target, {
      headers,
      redirect: 'manual',
      signal,
    });

    if (!REDIRECT_STATUSES.has(response.status)) {
      return { response, finalUrl: target };
    }

    const location = response.headers.get('location');
    if (!location) return { response, finalUrl: target };
    if (redirectCount === MAX_REDIRECTS) {
      await response.body?.cancel();
      throw new ProxyRequestError(502, 'Too many upstream redirects');
    }

    let redirectedTarget: URL;
    try {
      redirectedTarget = new URL(location, target);
    } catch {
      await response.body?.cancel();
      throw new ProxyRequestError(502, 'Invalid upstream redirect');
    }
    await response.body?.cancel();
    target = redirectedTarget;
  }

  throw new ProxyRequestError(502, 'Too many upstream redirects');
}

function buildProxyUrl(
  target: URL,
  customHeaders: Record<string, string>,
): string {
  const params = new URLSearchParams({ url: target.toString() });
  if (Object.keys(customHeaders).length > 0) {
    params.set('headers', JSON.stringify(customHeaders));
  }
  return `/stream-proxy?${params}`;
}

function proxyHlsUri(
  uri: string,
  baseUrl: URL,
  customHeaders: Record<string, string>,
): string {
  let target: URL;
  try {
    target = new URL(uri, baseUrl);
  } catch {
    return uri;
  }
  if (target.protocol !== 'http:' && target.protocol !== 'https:') return uri;
  return buildProxyUrl(target, customHeaders);
}

function rewriteUriAttributes(
  line: string,
  baseUrl: URL,
  customHeaders: Record<string, string>,
): string {
  return line.replace(
    /(^|[:,])(\s*URI\s*=\s*")([^"]*)(")/g,
    (_match, separator: string, prefix: string, uri: string, quote: string) =>
      `${separator}${prefix}${proxyHlsUri(uri, baseUrl, customHeaders)}${quote}`,
  );
}

function rewriteHlsLine(
  line: string,
  baseUrl: URL,
  customHeaders: Record<string, string>,
): string {
  const trimmed = line.trim();
  if (!trimmed) return line;
  if (trimmed.startsWith('#')) {
    return rewriteUriAttributes(line, baseUrl, customHeaders);
  }

  const start = line.indexOf(trimmed);
  const end = start + trimmed.length;
  return `${line.slice(0, start)}${proxyHlsUri(trimmed, baseUrl, customHeaders)}${line.slice(end)}`;
}

export function rewriteHlsManifest(
  manifest: string,
  baseUrl: URL,
  customHeaders: Record<string, string>,
): string {
  return manifest
    .split(/(\r\n|\n|\r)/)
    .map((part) =>
      part === '\r\n' || part === '\n' || part === '\r'
        ? part
        : rewriteHlsLine(part, baseUrl, customHeaders),
    )
    .join('');
}

function looksLikeHlsManifest(bytes: Uint8Array): boolean {
  let offset = 0;
  if (bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) {
    offset = 3;
  }
  const magic = [0x23, 0x45, 0x58, 0x54, 0x4d, 0x33, 0x55];
  return magic.every((byte, index) => bytes[offset + index] === byte);
}

function isHlsContentType(contentType: string | null): boolean {
  if (!contentType) return false;
  const mediaType = contentType.split(';', 1)[0]?.trim().toLowerCase();
  return (
    mediaType === 'application/vnd.apple.mpegurl' ||
    mediaType === 'application/x-mpegurl' ||
    mediaType === 'audio/mpegurl' ||
    mediaType === 'audio/x-mpegurl'
  );
}

function copyResponseHeaders(upstream: Response): Headers {
  const headers = new Headers();
  for (const name of [
    'content-type',
    'content-range',
    'accept-ranges',
    'cache-control',
    'etag',
    'last-modified',
    'date',
  ]) {
    const value = upstream.headers.get(name);
    if (value) headers.set(name, value);
  }
  return headers;
}

async function readManifest(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  initialChunks: Uint8Array[],
): Promise<Uint8Array> {
  const chunks = [...initialChunks];
  let total = chunks.reduce((size, chunk) => size + chunk.byteLength, 0);
  if (total > MAX_MANIFEST_BYTES) {
    await reader.cancel('HLS manifest exceeds size limit');
    throw new ProxyRequestError(502, 'Upstream HLS manifest is too large');
  }

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    if (!value) continue;
    total += value.byteLength;
    if (total > MAX_MANIFEST_BYTES) {
      await reader.cancel('HLS manifest exceeds size limit');
      throw new ProxyRequestError(502, 'Upstream HLS manifest is too large');
    }
    chunks.push(value);
  }

  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

async function createProxyResponse(
  upstream: Response,
  finalUrl: URL,
  customHeaders: Record<string, string>,
  abortController: AbortController,
): Promise<Response> {
  const responseHeaders = copyResponseHeaders(upstream);
  if (!upstream.body) {
    return new Response(null, {
      status: upstream.status,
      statusText: upstream.statusText,
      headers: responseHeaders,
    });
  }

  const reader = upstream.body.getReader();
  const initialChunks: Uint8Array[] = [];
  let initialSize = 0;
  while (initialSize < HLS_MAGIC_SCAN_BYTES) {
    const { done, value } = await reader.read();
    if (done) break;
    if (!value) continue;
    initialChunks.push(value);
    initialSize += value.byteLength;
  }

  const prefix = new Uint8Array(initialSize);
  let prefixOffset = 0;
  for (const chunk of initialChunks) {
    prefix.set(chunk, prefixOffset);
    prefixOffset += chunk.byteLength;
  }

  if (
    looksLikeHlsManifest(prefix) ||
    isHlsContentType(upstream.headers.get('content-type'))
  ) {
    const bytes = await readManifest(reader, initialChunks);
    if (!looksLikeHlsManifest(bytes)) {
      return new Response(bytes, {
        status: upstream.status,
        statusText: upstream.statusText,
        headers: responseHeaders,
      });
    }

    let manifest: string;
    try {
      manifest = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    } catch {
      throw new ProxyRequestError(502, 'Upstream HLS manifest is not UTF-8');
    }
    const rewritten = rewriteHlsManifest(manifest, finalUrl, customHeaders);
    responseHeaders.delete('content-range');
    responseHeaders.delete('accept-ranges');
    responseHeaders.delete('etag');
    responseHeaders.delete('last-modified');
    responseHeaders.set(
      'content-type',
      upstream.headers.get('content-type') ||
        'application/vnd.apple.mpegurl; charset=utf-8',
    );
    return new Response(rewritten, {
      status: upstream.status,
      statusText: upstream.statusText,
      headers: responseHeaders,
    });
  }

  let initialChunkIndex = 0;
  const body = new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (initialChunkIndex < initialChunks.length) {
        controller.enqueue(initialChunks[initialChunkIndex]);
        initialChunkIndex += 1;
        return;
      }

      try {
        const { done, value } = await reader.read();
        if (done) {
          controller.close();
        } else if (value) {
          controller.enqueue(value);
        }
      } catch (error) {
        if (abortController.signal.aborted) {
          controller.close();
        } else {
          controller.error(error);
        }
      }
    },
    async cancel(reason) {
      abortController.abort(reason);
      await reader.cancel(reason);
    },
  });

  return new Response(body, {
    status: upstream.status,
    statusText: upstream.statusText,
    headers: responseHeaders,
  });
}

function errorResponse(error: unknown): Response {
  if (error instanceof ProxyRequestError) {
    return new Response(error.message, { status: error.status });
  }
  if (error instanceof Error && error.name === 'AbortError') {
    return new Response(null, { status: 499 });
  }

  console.error('[StreamProxy] Upstream request failed', {
    errorType: error instanceof Error ? error.name : typeof error,
  });
  return new Response('Proxy request failed', { status: 502 });
}

export async function handleStreamProxyRequest(
  request: Request,
): Promise<Response> {
  try {
    const user = await ensureValidToken();
    if (!user) return new Response('Unauthorized', { status: 401 });

    const requestUrl = new URL(request.url);
    const rawTarget = requestUrl.searchParams.get('url');
    if (!rawTarget) {
      return new Response('Missing url parameter', { status: 400 });
    }

    const customHeaders = parseCustomHeaders(
      requestUrl.searchParams.get('headers'),
    );
    const target = await validateProxyTarget(rawTarget);
    const upstreamHeaders = buildUpstreamHeaders(customHeaders, request);
    const abortController = new AbortController();
    if (request.signal.aborted) {
      abortController.abort(request.signal.reason);
    } else {
      request.signal.addEventListener(
        'abort',
        () => abortController.abort(request.signal.reason),
        { once: true },
      );
    }

    const { response, finalUrl } = await fetchWithValidatedRedirects(
      target,
      upstreamHeaders,
      abortController.signal,
    );
    return await createProxyResponse(
      response,
      finalUrl,
      customHeaders,
      abortController,
    );
  } catch (error) {
    return errorResponse(error);
  }
}
