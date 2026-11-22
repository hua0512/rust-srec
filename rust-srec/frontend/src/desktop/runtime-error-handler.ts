export const FALLBACK_ERROR_MESSAGE = 'An unexpected error occurred';
const DEFAULT_DEDUPE_WINDOW_MS = 5_000;

const IGNORABLE_ERROR_PATTERNS = [
  /ResizeObserver loop completed with undelivered notifications/i,
  /ResizeObserver loop limit exceeded/i,
];

function readRawErrorMessage(error: unknown): string | null {
  if (typeof error === 'string') {
    return error;
  }

  if (error instanceof Error) {
    return error.message;
  }

  if (typeof ErrorEvent !== 'undefined' && error instanceof ErrorEvent) {
    if (error.error != null) {
      const inner = readRawErrorMessage(error.error);
      if (inner && inner.trim().length > 0) {
        return inner;
      }
    }
    return error.message;
  }

  if (typeof error === 'object' && error !== null && 'message' in error) {
    const candidate = (error as { message: unknown }).message;
    if (typeof candidate === 'string') {
      return candidate;
    }
  }

  return null;
}

/**
 * Returns true for benign browser notifications (such as ResizeObserver loop
 * warnings) that should neither crash boot nor surface user-facing error toasts.
 */
export function isIgnorableRuntimeError(error: unknown): boolean {
  const raw = readRawErrorMessage(error);
  if (!raw) {
    return false;
  }

  return IGNORABLE_ERROR_PATTERNS.some((pattern) => pattern.test(raw));
}

/**
 * Extracts a concise single-line user-facing message from an arbitrary thrown
 * value or browser error event.
 */
export function extractErrorMessage(error: unknown): string {
  const raw = readRawErrorMessage(error);
  if (!raw) {
    return FALLBACK_ERROR_MESSAGE;
  }

  const firstLine = raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line.length > 0);

  return firstLine ?? FALLBACK_ERROR_MESSAGE;
}

export interface DesktopErrorControllerOptions {
  onBootFatal: (error: unknown) => void;
  onRuntimeError: (message: string, error: unknown) => void;
  now?: () => number;
  dedupeWindowMs?: number;
}

export interface DesktopErrorController {
  markMounted: () => void;
  handleError: (error: unknown) => void;
}

/**
 * Coordinates desktop global error handling across boot and mounted phases.
 *
 * Errors before mount escalate to the fatal recovery screen at most once so
 * cascading startup failures do not thrash it. After mount, non-ignorable
 * errors are deduplicated by message within `dedupeWindowMs` and routed to a
 * non-destructive runtime notification handler.
 */
export function createDesktopErrorController(
  options: DesktopErrorControllerOptions,
): DesktopErrorController {
  const {
    onBootFatal,
    onRuntimeError,
    now = Date.now,
    dedupeWindowMs = DEFAULT_DEDUPE_WINDOW_MS,
  } = options;

  let phase: 'booting' | 'mounted' = 'booting';
  let bootFatalTriggered = false;
  const recentMessages = new Map<string, number>();

  return {
    markMounted() {
      phase = 'mounted';
    },

    handleError(error: unknown) {
      if (isIgnorableRuntimeError(error)) {
        return;
      }

      if (phase === 'booting') {
        if (bootFatalTriggered) {
          return;
        }
        bootFatalTriggered = true;
        onBootFatal(error);
        return;
      }

      const message = extractErrorMessage(error);
      const timestamp = now();

      for (const [seenMessage, seenAt] of recentMessages) {
        if (timestamp - seenAt >= dedupeWindowMs) {
          recentMessages.delete(seenMessage);
        }
      }

      const lastShownAt = recentMessages.get(message);
      if (
        lastShownAt !== undefined &&
        timestamp - lastShownAt < dedupeWindowMs
      ) {
        return;
      }

      recentMessages.set(message, timestamp);
      onRuntimeError(message, error);
    },
  };
}
