/**
 * Protocol Buffer types and codec for download progress WebSocket messages.
 * Generated from rust-srec/proto/download_progress.proto
 */
import { download_progress as pb } from './download_progress_pb.js';

// Event types for WebSocket messages
export enum EventType {
  EVENT_TYPE_UNSPECIFIED = 0,
  EVENT_TYPE_SNAPSHOT = 1,
  EVENT_TYPE_DOWNLOAD_STARTED = 2,
  EVENT_TYPE_PROGRESS = 3,
  EVENT_TYPE_SEGMENT_COMPLETED = 4,
  EVENT_TYPE_DOWNLOAD_COMPLETED = 5,
  EVENT_TYPE_DOWNLOAD_FAILED = 6,
  EVENT_TYPE_DOWNLOAD_CANCELLED = 7,
  EVENT_TYPE_ERROR = 8,
  EVENT_TYPE_DOWNLOAD_REJECTED = 9,
}

// Progress information for a single download
export interface DownloadProgress {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  engineType: string;
  status: string;
  bytesDownloaded: bigint;
  durationSecs: number;
  speedBytesPerSec: bigint;
  segmentsCompleted: number;
  mediaDurationSecs: number;
  playbackRatio: number;
  startedAtMs: bigint;
}

// Initial snapshot of all active downloads
export interface DownloadSnapshot {
  downloads: DownloadProgress[];
}

// Download started event
export interface DownloadStarted {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  engineType: string;
  startedAtMs: bigint;
}

// Segment completed event
export interface SegmentCompleted {
  downloadId: string;
  streamerId: string;
  segmentPath: string;
  segmentIndex: number;
  durationSecs: number;
  sizeBytes: bigint;
  sessionId: string;
}

// Download completed event
export interface DownloadCompleted {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  totalBytes: bigint;
  totalDurationSecs: number;
  totalSegments: number;
}

// Download failed event
export interface DownloadFailed {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  error: string;
  recoverable: boolean;
}

// Download cancelled event
export interface DownloadCancelled {
  downloadId: string;
  streamerId: string;
  sessionId: string;
  cause: string;
}

// Download rejected event
export interface DownloadRejected {
  streamerId: string;
  sessionId: string;
  reason: string;
  retryAfterSecs: bigint;
  recoverable: boolean;
}

// Error payload for service errors
export interface ErrorPayload {
  code: string;
  message: string;
}

// Server-to-client message payload union type
export type WsMessagePayload =
  | { snapshot: DownloadSnapshot }
  | { downloadStarted: DownloadStarted }
  | { progress: DownloadProgress }
  | { segmentCompleted: SegmentCompleted }
  | { downloadCompleted: DownloadCompleted }
  | { downloadFailed: DownloadFailed }
  | { downloadCancelled: DownloadCancelled }
  | { downloadRejected: DownloadRejected }
  | { error: ErrorPayload };

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
  if (typeof value === 'bigint') return value;
  if (typeof value === 'number') return BigInt(value);
  if (typeof value === 'string') return BigInt(value);
  // Handle protobufjs Long type
  if (value && typeof value === 'object' && 'low' in value && 'high' in value) {
    const long = value as { low: number; high: number; unsigned?: boolean };
    const low = BigInt(long.low >>> 0);
    const high = BigInt(long.high >>> 0);
    return (high << 32n) | low;
  }
  return 0n;
}

/**
 * Convert a raw protobuf DownloadProgress to our interface
 */
function convertDownloadProgress(
  raw: Record<string, unknown>,
): DownloadProgress {
  return {
    downloadId: (raw.downloadId as string) || '',
    streamerId: (raw.streamerId as string) || '',
    sessionId: (raw.sessionId as string) || '',
    engineType: (raw.engineType as string) || '',
    status: (raw.status as string) || '',
    bytesDownloaded: toBigInt(raw.bytesDownloaded),
    durationSecs: (raw.durationSecs as number) || 0,
    speedBytesPerSec: toBigInt(raw.speedBytesPerSec),
    segmentsCompleted: (raw.segmentsCompleted as number) || 0,
    mediaDurationSecs: (raw.mediaDurationSecs as number) || 0,
    playbackRatio: (raw.playbackRatio as number) || 0,
    startedAtMs: toBigInt(raw.startedAtMs),
  };
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
        downloads: rawDownloads.map(convertDownloadProgress),
      },
    };
  } else if (decoded.downloadStarted) {
    const raw = decoded.downloadStarted as Record<string, unknown>;
    payload = {
      downloadStarted: {
        downloadId: (raw.downloadId as string) || '',
        streamerId: (raw.streamerId as string) || '',
        sessionId: (raw.sessionId as string) || '',
        engineType: (raw.engineType as string) || '',
        startedAtMs: toBigInt(raw.startedAtMs),
      },
    };
  } else if (decoded.progress) {
    payload = {
      progress: convertDownloadProgress(
        decoded.progress as Record<string, unknown>,
      ),
    };
  } else if (decoded.segmentCompleted) {
    const raw = decoded.segmentCompleted as Record<string, unknown>;
    payload = {
      segmentCompleted: {
        downloadId: (raw.downloadId as string) || '',
        streamerId: (raw.streamerId as string) || '',
        segmentPath: (raw.segmentPath as string) || '',
        segmentIndex: (raw.segmentIndex as number) || 0,
        durationSecs: (raw.durationSecs as number) || 0,
        sizeBytes: toBigInt(raw.sizeBytes),
        sessionId: (raw.sessionId as string) || '',
      },
    };
  } else if (decoded.downloadCompleted) {
    const raw = decoded.downloadCompleted as Record<string, unknown>;
    payload = {
      downloadCompleted: {
        downloadId: (raw.downloadId as string) || '',
        streamerId: (raw.streamerId as string) || '',
        sessionId: (raw.sessionId as string) || '',
        totalBytes: toBigInt(raw.totalBytes),
        totalDurationSecs: (raw.totalDurationSecs as number) || 0,
        totalSegments: (raw.totalSegments as number) || 0,
      },
    };
  } else if (decoded.downloadFailed) {
    const raw = decoded.downloadFailed as Record<string, unknown>;
    payload = {
      downloadFailed: {
        downloadId: (raw.downloadId as string) || '',
        streamerId: (raw.streamerId as string) || '',
        sessionId: (raw.sessionId as string) || '',
        error: (raw.error as string) || '',
        recoverable: (raw.recoverable as boolean) || false,
      },
    };
  } else if (decoded.downloadCancelled) {
    const raw = decoded.downloadCancelled as Record<string, unknown>;
    payload = {
      downloadCancelled: {
        downloadId: (raw.downloadId as string) || '',
        streamerId: (raw.streamerId as string) || '',
        sessionId: (raw.sessionId as string) || '',
        cause: (raw.cause as string) || '',
      },
    };
  } else if (decoded.downloadRejected) {
    const raw = decoded.downloadRejected as Record<string, unknown>;
    payload = {
      downloadRejected: {
        streamerId: (raw.streamerId as string) || '',
        sessionId: (raw.sessionId as string) || '',
        reason: (raw.reason as string) || '',
        retryAfterSecs: toBigInt(raw.retryAfterSecs),
        recoverable: (raw.recoverable as boolean) || false,
      },
    };
  } else if (decoded.error) {
    const raw = decoded.error as Record<string, unknown>;
    payload = {
      error: {
        code: (raw.code as string) || '',
        message: (raw.message as string) || '',
      },
    };
  } else {
    // Default to empty error payload for unspecified
    payload = { error: { code: 'UNKNOWN', message: 'Unknown message type' } };
  }

  return { eventType, payload };
}

/**
 * Encode a ClientMessage to binary for sending to the server
 */
export function encodeClientMessage(msg: ClientMessage): Uint8Array {
  let protoMsg: Record<string, unknown>;

  if ('subscribe' in msg.action) {
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
