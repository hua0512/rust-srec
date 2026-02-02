/**
 * Protocol Buffer types and codec for download progress WebSocket messages.
 * Generated from rust-srec/proto/download_progress.proto
 */
// @ts-expect-error - Generated protobuf JS file without type declarations
import { download_progress as pb } from "./download_progress_pb.js";

// Event types for WebSocket messages
export enum EventType {
  EVENT_TYPE_UNSPECIFIED = 0,
  EVENT_TYPE_SNAPSHOT = 1,
  EVENT_TYPE_DOWNLOAD_META = 2,
  EVENT_TYPE_DOWNLOAD_METRICS = 3,
  EVENT_TYPE_SEGMENT_COMPLETED = 4,
  EVENT_TYPE_DOWNLOAD_COMPLETED = 5,
  EVENT_TYPE_DOWNLOAD_FAILED = 6,
  EVENT_TYPE_DOWNLOAD_CANCELLED = 7,
  EVENT_TYPE_ERROR = 8,
  EVENT_TYPE_DOWNLOAD_REJECTED = 9,
}

// Download metadata (low frequency)
export interface DownloadMeta {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  engineType: string;
  startedAtMs: bigint;
  // Monotonic (best-effort) meta update time.
  updatedAtMs: bigint;
  cdnHost: string;
  downloadUrl: string;
}

// Download metrics (high frequency)
export interface DownloadMetrics {
  downloadId: string;
  status: string;
  bytesDownloaded: bigint;
  durationSecs: number;
  speedBytesPerSec: bigint;
  segmentsCompleted: number;
  mediaDurationSecs: number;
  playbackRatio: number;
}

export interface DownloadState {
  meta: DownloadMeta;
  metrics: DownloadMetrics;
}

// Initial snapshot of all active downloads
export interface DownloadSnapshot {
  downloads: DownloadState[];
}

// Server-to-client message payload union type
export type WsMessagePayload =
  | { snapshot: DownloadSnapshot }
  | { downloadMeta: DownloadMeta }
  | { downloadMetrics: DownloadMetrics }
  | { segmentCompleted: SegmentCompleted }
  | { downloadCompleted: DownloadCompleted }
  | { downloadFailed: DownloadFailed }
  | { downloadCancelled: DownloadCancelled }
  | { downloadRejected: DownloadRejected }
  | { error: ErrorPayload };

export interface SegmentCompleted {
  downloadId: string;
  streamerId: string;
  segmentPath: string;
  segmentIndex: number;
  durationSecs: number;
  sizeBytes: bigint;
  sessionId: string;
}

export interface DownloadCompleted {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  totalBytes: bigint;
  totalDurationSecs: number;
  totalSegments: number;
}

export interface DownloadFailed {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  error: string;
  recoverable: boolean;
}

export interface DownloadCancelled {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  cause: string;
}

export interface DownloadRejected {
  streamerId: string;
  sessionId: string;
  reason: string;
  retryAfterSecs: bigint;
  recoverable: boolean;
}

export interface ErrorPayload {
  code: string;
  message: string;
}

// Server-to-client message envelope
export interface WsMessage {
  eventType: EventType;
  payload: WsMessagePayload;
}

// Subscribe request
export interface SubscribeRequest {
  streamerId: string;
}

// Unsubscribe request
export interface UnsubscribeRequest {}

// Client-to-server message
export interface ClientMessage {
  action: { subscribe: SubscribeRequest } | { unsubscribe: UnsubscribeRequest };
}

// Precompiled protobufjs codec.
// Avoids runtime schema parsing (which relies on codegen / unsafe-eval).
const WsMessageType = pb.WsMessage;
const ClientMessageType = pb.ClientMessage;

/**
 * Convert protobuf Long to bigint
 */
function toBigInt(value: unknown): bigint {
  if (typeof value === "bigint") return value;
  if (typeof value === "number") return BigInt(value);
  if (typeof value === "string") return BigInt(value);
  // Handle protobufjs Long type
  if (value && typeof value === "object" && "low" in value && "high" in value) {
    const long = value as { low: number; high: number; unsigned?: boolean };
    const low = BigInt(long.low >>> 0);
    const high = BigInt(long.high >>> 0);
    return (high << 32n) | low;
  }
  return 0n;
}

function convertDownloadMeta(raw: Record<string, unknown>): DownloadMeta {
  return {
    downloadId: (raw.downloadId as string) || "",
    streamerId: (raw.streamerId as string) || "",
    sessionId: (raw.sessionId as string) || "",
    engineType: (raw.engineType as string) || "",
    startedAtMs: toBigInt(raw.startedAtMs),
    updatedAtMs: toBigInt(raw.updatedAtMs),
    cdnHost: (raw.cdnHost as string) || "",
    downloadUrl: (raw.downloadUrl as string) || "",
  };
}

function convertDownloadMetrics(raw: Record<string, unknown>): DownloadMetrics {
  return {
    downloadId: (raw.downloadId as string) || "",
    status: (raw.status as string) || "",
    bytesDownloaded: toBigInt(raw.bytesDownloaded),
    durationSecs: (raw.durationSecs as number) || 0,
    speedBytesPerSec: toBigInt(raw.speedBytesPerSec),
    segmentsCompleted: (raw.segmentsCompleted as number) || 0,
    mediaDurationSecs: (raw.mediaDurationSecs as number) || 0,
    playbackRatio: (raw.playbackRatio as number) || 0,
  };
}

function convertSegmentCompleted(raw: Record<string, unknown>): SegmentCompleted {
  return {
    downloadId: (raw.downloadId as string) || "",
    streamerId: (raw.streamerId as string) || "",
    segmentPath: (raw.segmentPath as string) || "",
    segmentIndex: (raw.segmentIndex as number) || 0,
    durationSecs: (raw.durationSecs as number) || 0,
    sizeBytes: toBigInt(raw.sizeBytes),
    sessionId: (raw.sessionId as string) || "",
  };
}

function convertDownloadCompleted(raw: Record<string, unknown>): DownloadCompleted {
  return {
    downloadId: (raw.downloadId as string) || "",
    streamerId: (raw.streamerId as string) || "",
    sessionId: (raw.sessionId as string) || "",
    totalBytes: toBigInt(raw.totalBytes),
    totalDurationSecs: (raw.totalDurationSecs as number) || 0,
    totalSegments: (raw.totalSegments as number) || 0,
  };
}

function convertDownloadFailed(raw: Record<string, unknown>): DownloadFailed {
  return {
    downloadId: (raw.downloadId as string) || "",
    streamerId: (raw.streamerId as string) || "",
    sessionId: (raw.sessionId as string) || "",
    error: (raw.error as string) || "",
    recoverable: (raw.recoverable as boolean) || false,
  };
}

function convertDownloadCancelled(raw: Record<string, unknown>): DownloadCancelled {
  return {
    downloadId: (raw.downloadId as string) || "",
    streamerId: (raw.streamerId as string) || "",
    sessionId: (raw.sessionId as string) || "",
    cause: (raw.cause as string) || "",
  };
}

function convertDownloadRejected(raw: Record<string, unknown>): DownloadRejected {
  return {
    streamerId: (raw.streamerId as string) || "",
    sessionId: (raw.sessionId as string) || "",
    reason: (raw.reason as string) || "",
    retryAfterSecs: toBigInt(raw.retryAfterSecs),
    recoverable: (raw.recoverable as boolean) || false,
  };
}

function convertErrorPayload(raw: Record<string, unknown>): ErrorPayload {
  return {
    code: (raw.code as string) || "",
    message: (raw.message as string) || "",
  };
}

function convertDownloadState(raw: Record<string, unknown>): DownloadState {
  const metaRaw = raw.meta as Record<string, unknown> | undefined;
  const metricsRaw = raw.metrics as Record<string, unknown> | undefined;

  // In proto3, non-optional message fields can still be omitted; be defensive.
  const meta = metaRaw ? convertDownloadMeta(metaRaw) : convertDownloadMeta({});
  const metrics = metricsRaw ? convertDownloadMetrics(metricsRaw) : convertDownloadMetrics({});

  // If one side has an id and the other doesn't, align them.
  if (!meta.downloadId && metrics.downloadId) meta.downloadId = metrics.downloadId;
  if (!metrics.downloadId && meta.downloadId) metrics.downloadId = meta.downloadId;

  return { meta, metrics };
}

/**
 * Decode a binary WebSocket message to WsMessage
 */
export function decodeWsMessage(data: Uint8Array): WsMessage {
  const decoded = WsMessageType.decode(data) as unknown as Record<
    string,
    unknown
  >;
  const eventType =
    (decoded.eventType as number) || EventType.EVENT_TYPE_UNSPECIFIED;

  let payload: WsMessagePayload;

  if (decoded.snapshot) {
    const rawSnapshot = decoded.snapshot as Record<string, unknown>;
    const rawDownloads =
      (rawSnapshot.downloads as Record<string, unknown>[]) || [];
    payload = {
      snapshot: {
        downloads: rawDownloads.map(convertDownloadState),
      },
    };
  } else if (decoded.downloadMeta) {
    const raw = decoded.downloadMeta as Record<string, unknown>;
    payload = {
      downloadMeta: convertDownloadMeta(raw),
    };
  } else if (decoded.downloadMetrics) {
    const raw = decoded.downloadMetrics as Record<string, unknown>;
    payload = {
      downloadMetrics: convertDownloadMetrics(raw),
    };
  } else if (decoded.segmentCompleted) {
    const raw = decoded.segmentCompleted as Record<string, unknown>;
    payload = {
      segmentCompleted: convertSegmentCompleted(raw),
    };
  } else if (decoded.downloadCompleted) {
    const raw = decoded.downloadCompleted as Record<string, unknown>;
    payload = {
      downloadCompleted: convertDownloadCompleted(raw),
    };
  } else if (decoded.downloadFailed) {
    const raw = decoded.downloadFailed as Record<string, unknown>;
    payload = {
      downloadFailed: convertDownloadFailed(raw),
    };
  } else if (decoded.downloadCancelled) {
    const raw = decoded.downloadCancelled as Record<string, unknown>;
    payload = {
      downloadCancelled: convertDownloadCancelled(raw),
    };
  } else if (decoded.downloadRejected) {
    const raw = decoded.downloadRejected as Record<string, unknown>;
    payload = {
      downloadRejected: convertDownloadRejected(raw),
    };
  } else if (decoded.error) {
    const raw = decoded.error as Record<string, unknown>;
    payload = {
      error: convertErrorPayload(raw),
    };
  } else {
    // Default to unknown message type
    throw new Error("Unknown WebSocket message type");
  }

  return { eventType, payload };
}

/**
 * Encode a ClientMessage to binary for sending to the server
 */
export function encodeClientMessage(msg: ClientMessage): Uint8Array {
  let protoMsg: Record<string, unknown>;

  if ("subscribe" in msg.action) {
    protoMsg = {
      subscribe: {
        streamerId: msg.action.subscribe.streamerId,
      },
    };
  } else {
    protoMsg = {
      unsubscribe: {},
    };
  }

  const errMsg = ClientMessageType.verify(protoMsg);
  if (errMsg) {
    throw new Error(`Invalid ClientMessage: ${errMsg}`);
  }

  const message = ClientMessageType.create(protoMsg);
  return ClientMessageType.encode(message).finish();
}

// Export the protobuf types for testing
export { WsMessageType, ClientMessageType };
