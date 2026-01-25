/**
 * Protocol Buffer types and codec for log event WebSocket messages.
 * Generated from rust-srec/proto/log_event.proto
 */
import { log_event as pb } from './log_event_pb.js';

// Log severity levels
export enum LogLevel {
  LOG_LEVEL_UNSPECIFIED = 0,
  LOG_LEVEL_TRACE = 1,
  LOG_LEVEL_DEBUG = 2,
  LOG_LEVEL_INFO = 3,
  LOG_LEVEL_WARN = 4,
  LOG_LEVEL_ERROR = 5,
}

// Event types for WebSocket messages
export enum EventType {
  EVENT_TYPE_UNSPECIFIED = 0,
  EVENT_TYPE_LOG = 1,
  EVENT_TYPE_ERROR = 2,
}

// A single log event
export interface LogEvent {
  timestampMs: bigint;
  level: LogLevel;
  target: string;
  message: string;
}

// Error payload for service errors
export interface ErrorPayload {
  code: string;
  message: string;
}

// Server-to-client message payload union type
export type WsMessagePayload = { log: LogEvent } | { error: ErrorPayload };

// Server-to-client message envelope
export interface WsMessage {
  eventType: EventType;
  payload: WsMessagePayload;
}

// Precompiled protobufjs codec.
// Avoids runtime schema parsing (which relies on codegen / unsafe-eval).
const WsMessageType = pb.WsMessage;

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

  if (decoded.log) {
    const raw = decoded.log as Record<string, unknown>;
    payload = {
      log: {
        timestampMs: toBigInt(raw.timestampMs),
        level: (raw.level as LogLevel) || LogLevel.LOG_LEVEL_UNSPECIFIED,
        target: (raw.target as string) || '',
        message: (raw.message as string) || '',
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
 * Get human-readable log level name
 */
export function getLogLevelName(level: LogLevel): string {
  switch (level) {
    case LogLevel.LOG_LEVEL_TRACE:
      return 'TRACE';
    case LogLevel.LOG_LEVEL_DEBUG:
      return 'DEBUG';
    case LogLevel.LOG_LEVEL_INFO:
      return 'INFO';
    case LogLevel.LOG_LEVEL_WARN:
      return 'WARN';
    case LogLevel.LOG_LEVEL_ERROR:
      return 'ERROR';
    default:
      return 'UNKNOWN';
  }
}

// Export the protobuf type for testing
export { WsMessageType };
