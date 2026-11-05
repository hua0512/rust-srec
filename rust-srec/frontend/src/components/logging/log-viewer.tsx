/**
 * Real-time log viewer component that consumes the log streaming WebSocket.
 */
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useRouteContext } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { msg, plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import {
  EventType,
  LogLevel,
  WsMessageSchema,
  type LogEvent,
} from '@/api/proto/gen/log_event_pb.js';
import { fromBinary } from '@bufbuild/protobuf';
import { sessionQueryOptions } from '@/api/session';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Terminal,
  Pause,
  Play,
  Trash2,
  Search,
  ArrowDown,
  Wifi,
  WifiOff,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { useAuthedWebSocket } from '@/hooks/use-authed-websocket';
import {
  LogLevelIcon,
  fromProtoLogLevel,
  logLevelStyle,
} from '@/components/logging/log-level-style';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';

const MAX_LOG_ENTRIES = 500;

/**
 * At DEBUG/TRACE volume frames arrive far faster than the list can usefully
 * repaint, so they are collected in a ref and handed to React on this cadence:
 * one state update per window instead of one per frame, which keeps the cost of
 * a frame independent of how many rows are on screen. A timer rather than an
 * animation frame, so a backgrounded tab still drains its buffer instead of
 * growing it until the tab is looked at again.
 */
const LOG_FLUSH_INTERVAL_MS = 50;

interface DisplayLogEvent extends LogEvent {
  id: number;
}

type FilterLevel = 'all' | 'trace' | 'debug' | 'info' | 'warn' | 'error';

/** Lowest level each filter keeps; higher levels always pass. */
const FILTER_LEVEL_FLOOR: Record<FilterLevel, LogLevel> = {
  all: LogLevel.UNSPECIFIED,
  trace: LogLevel.TRACE,
  debug: LogLevel.DEBUG,
  info: LogLevel.INFO,
  warn: LogLevel.WARN,
  error: LogLevel.ERROR,
};

function formatTime(timestampMs: bigint): string {
  const date = new Date(Number(timestampMs));
  return date.toLocaleTimeString('en-US', {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  });
}

/** Keeps the newest entries once the list is over its cap. */
function capToLimit(entries: DisplayLogEvent[]): DisplayLogEvent[] {
  return entries.length > MAX_LOG_ENTRIES
    ? entries.slice(-MAX_LOG_ENTRIES)
    : entries;
}

/**
 * A single log line. Memoized on the entry, which never changes once decoded,
 * so appending new lines leaves the rows already on screen untouched.
 */
const LogRow = memo(function LogRow({
  log,
  animate,
}: {
  log: DisplayLogEvent;
  animate: boolean;
}) {
  const level = fromProtoLogLevel(log.level);
  const style = logLevelStyle(level);
  return (
    <motion.div
      initial={animate ? { opacity: 0, x: -10 } : false}
      animate={animate ? { opacity: 1, x: 0 } : undefined}
      transition={{ duration: 0.1 }}
      className={cn(
        'flex items-start gap-2 px-3 py-1.5 border-b border-border/20 transition-colors',
        style.rowClassName,
      )}
    >
      <span className="text-muted-foreground shrink-0 w-21.25">
        {formatTime(log.timestampMs)}
      </span>
      <Badge
        variant="outline"
        className={cn(
          'text-[9px] uppercase font-medium shrink-0 px-1.5 py-0',
          style.badgeClassName,
        )}
      >
        <LogLevelIcon level={level} />
        <span className="ml-1">{level ?? 'unknown'}</span>
      </Badge>
      <span className="text-primary/80 shrink-0 max-w-37.5 truncate">
        {log.target}
      </span>
      <span className="text-foreground/90 break-all flex-1">{log.message}</span>
    </motion.div>
  );
});

export function LogViewer() {
  const { i18n } = useLingui();
  const prefersReducedMotion = usePrefersReducedMotion();
  const [logs, setLogs] = useState<DisplayLogEvent[]>([]);
  const [isPaused, setIsPaused] = useState(false);
  const [pausedCount, setPausedCount] = useState(0);
  const [filterLevel, setFilterLevel] = useState<FilterLevel>('all');
  const [searchQuery, setSearchQuery] = useState('');
  const [autoScroll, setAutoScroll] = useState(true);

  const logContainerRef = useRef<HTMLDivElement>(null);
  const logIdRef = useRef(0);
  const pendingLogsRef = useRef<DisplayLogEvent[]>([]);
  const flushTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  // Read by the socket handler, which must see a pause the moment it is
  // requested rather than on the next render.
  const isPausedRef = useRef(false);
  const pausedLogsRef = useRef<DisplayLogEvent[]>([]);

  const { user: routeUser } = useRouteContext({ from: '/_authed' });
  const { data: sessionData } = useQuery({
    ...sessionQueryOptions,
    enabled: typeof window !== 'undefined',
    initialData: routeUser ?? null,
  });
  const accessToken = sessionData?.token?.access_token;

  const cancelFlush = useCallback(() => {
    if (flushTimeoutRef.current !== undefined) {
      clearTimeout(flushTimeoutRef.current);
      flushTimeoutRef.current = undefined;
    }
  }, []);

  /** Moves everything buffered since the last window into the view. */
  const flushPending = useCallback(() => {
    flushTimeoutRef.current = undefined;
    const pending = pendingLogsRef.current;
    if (pending.length === 0) return;
    pendingLogsRef.current = [];

    if (isPausedRef.current) {
      pausedLogsRef.current = capToLimit(pausedLogsRef.current.concat(pending));
      setPausedCount(pausedLogsRef.current.length);
      return;
    }
    setLogs((prev) => capToLimit(prev.concat(pending)));
  }, []);

  const handleMessage = useCallback(
    (event: MessageEvent) => {
      try {
        const data = new Uint8Array(event.data as ArrayBuffer);
        const message = fromBinary(WsMessageSchema, data);

        if (
          message.eventType === EventType.LOG &&
          message.payload.case === 'log'
        ) {
          const logEvent: DisplayLogEvent = {
            ...message.payload.value,
            id: logIdRef.current++,
          };

          pendingLogsRef.current.push(logEvent);
          // Nothing beyond a full screenful can ever be shown, so the buffer
          // stays bounded even if a burst outruns the flush window.
          if (pendingLogsRef.current.length > MAX_LOG_ENTRIES) {
            pendingLogsRef.current =
              pendingLogsRef.current.slice(-MAX_LOG_ENTRIES);
          }
          if (flushTimeoutRef.current === undefined) {
            flushTimeoutRef.current = setTimeout(
              flushPending,
              LOG_FLUSH_INTERVAL_MS,
            );
          }
        }
      } catch (error) {
        console.error('Failed to decode log message:', error);
      }
    },
    [flushPending],
  );

  const { status } = useAuthedWebSocket({
    accessToken,
    path: '/logging/stream',
    debugLabel: 'LOG WS',
    onMessage: handleMessage,
  });
  const isConnected = status === 'connected';

  // Drop whatever is still buffered when the viewer goes away; it has nowhere
  // left to be shown.
  useEffect(() => {
    return () => {
      cancelFlush();
      pendingLogsRef.current = [];
      pausedLogsRef.current = [];
    };
  }, [cancelFlush]);

  // Auto scroll to bottom
  useEffect(() => {
    if (autoScroll && logContainerRef.current && !isPaused) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [logs, autoScroll, isPaused]);

  // Handle pause/resume
  const togglePause = useCallback(() => {
    if (isPausedRef.current) {
      // Resume: add paused logs
      const buffered = pausedLogsRef.current;
      pausedLogsRef.current = [];
      isPausedRef.current = false;
      setLogs((prev) => capToLimit(prev.concat(buffered)));
      setPausedCount(0);
      setIsPaused(false);
      return;
    }
    // Frames that arrived before the pause belong to the live view, so they go
    // in before the buffer starts holding anything back.
    cancelFlush();
    flushPending();
    isPausedRef.current = true;
    setIsPaused(true);
  }, [cancelFlush, flushPending]);

  // Clear logs
  const clearLogs = useCallback(() => {
    cancelFlush();
    pendingLogsRef.current = [];
    pausedLogsRef.current = [];
    setLogs([]);
    setPausedCount(0);
  }, [cancelFlush]);

  // Filter logs - memoized to avoid recalculating on every render
  const filteredLogs = useMemo(() => {
    const floor = FILTER_LEVEL_FLOOR[filterLevel];
    const query = searchQuery.toLowerCase();
    return logs.filter((log) => {
      if (filterLevel !== 'all' && log.level < floor) return false;

      if (query) {
        return (
          log.target.toLowerCase().includes(query) ||
          log.message.toLowerCase().includes(query)
        );
      }

      return true;
    });
  }, [logs, filterLevel, searchQuery]);

  return (
    <Card className="border-border/40 bg-linear-to-b from-card to-card/80 shadow-lg">
      <CardHeader className="pb-4">
        <div className="flex flex-col gap-4">
          <div className="flex items-center justify-between">
            <div>
              <CardTitle className="flex items-center gap-2">
                <Terminal className="h-5 w-5 text-primary" />
                <Trans>Real-Time Logs</Trans>
                <Badge
                  variant="outline"
                  className={cn(
                    'ml-2 text-[10px]',
                    isConnected
                      ? 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20'
                      : 'bg-rose-500/10 text-rose-400 border-rose-500/20',
                  )}
                >
                  {isConnected ? (
                    <>
                      <Wifi className="w-3 h-3 mr-1" />
                      <Trans>Connected</Trans>
                    </>
                  ) : (
                    <>
                      <WifiOff className="w-3 h-3 mr-1" />
                      <Trans>Disconnected</Trans>
                    </>
                  )}
                </Badge>
              </CardTitle>
              <CardDescription className="mt-1.5">
                {t(
                  i18n,
                )`View application logs in real-time. Logs are limited to the last ${plural(MAX_LOG_ENTRIES, { one: '# entry', other: '# entries' })}.`}
              </CardDescription>
            </div>

            <div className="flex items-center gap-2">
              <Button
                variant={isPaused ? 'default' : 'outline'}
                size="sm"
                onClick={togglePause}
                className={cn(isPaused && 'animate-pulse')}
              >
                {isPaused ? (
                  <>
                    <Play className="w-4 h-4 mr-1" />
                    <Trans>Resume</Trans>
                  </>
                ) : (
                  <>
                    <Pause className="w-4 h-4 mr-1" />
                    <Trans>Pause</Trans>
                  </>
                )}
              </Button>
              <Button variant="outline" size="sm" onClick={clearLogs}>
                <Trash2 className="w-4 h-4 mr-1" />
                <Trans>Clear</Trans>
              </Button>
            </div>
          </div>

          {/* Filters */}
          <div className="flex flex-col sm:flex-row gap-3">
            <div className="relative flex-1">
              <Search
                aria-hidden="true"
                className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground"
              />
              <Input
                type="search"
                placeholder={i18n._(msg`Search logs...`)}
                aria-label={i18n._(msg`Search logs...`)}
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="pl-9"
              />
            </div>
            <Select
              value={filterLevel}
              onValueChange={(v) => setFilterLevel(v as FilterLevel)}
            >
              <SelectTrigger
                aria-label={i18n._(msg`Log level`)}
                className="w-full sm:w-35"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  <Trans>All Levels</Trans>
                </SelectItem>
                <SelectItem value="trace">TRACE+</SelectItem>
                <SelectItem value="debug">DEBUG+</SelectItem>
                <SelectItem value="info">INFO+</SelectItem>
                <SelectItem value="warn">WARN+</SelectItem>
                <SelectItem value="error">ERROR</SelectItem>
              </SelectContent>
            </Select>
            <Button
              variant={autoScroll ? 'default' : 'outline'}
              size="icon"
              onClick={() => setAutoScroll(!autoScroll)}
              title={
                autoScroll
                  ? i18n._(msg`Auto-scroll enabled`)
                  : i18n._(msg`Auto-scroll disabled`)
              }
            >
              <ArrowDown className="w-4 h-4" />
            </Button>
          </div>
        </div>
      </CardHeader>

      <CardContent>
        <div
          ref={logContainerRef}
          className="h-100 overflow-y-auto rounded-lg border border-border/40 bg-black/20 font-mono text-xs"
        >
          {filteredLogs.length === 0 ? (
            <div className="flex flex-col items-center justify-center h-full text-muted-foreground">
              <Terminal className="w-12 h-12 mb-4 opacity-30" />
              <p>
                <Trans>No logs to display</Trans>
              </p>
              {!isConnected && (
                <p className="text-xs mt-1 opacity-60">
                  <Trans>Waiting for connection...</Trans>
                </p>
              )}
            </div>
          ) : (
            filteredLogs.map((log) => (
              <LogRow key={log.id} log={log} animate={!prefersReducedMotion} />
            ))
          )}
        </div>

        {/* Status bar */}
        <div className="flex items-center justify-between mt-3 text-xs text-muted-foreground">
          <span>
            {t(
              i18n,
            )`${filteredLogs.length} / ${plural(logs.length, { one: '# entry', other: '# entries' })}`}
            {isPaused && pausedCount > 0 && (
              <span className="ml-2 text-amber-400">
                {t(i18n)`(+${pausedCount} paused)`}
              </span>
            )}
          </span>
          {isPaused && (
            <span className="text-amber-400 animate-pulse">
              <Trans>Logging paused</Trans>
            </span>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
