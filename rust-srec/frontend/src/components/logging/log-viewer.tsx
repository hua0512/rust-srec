/**
 * Real-time log viewer component that consumes the log streaming WebSocket.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useRouteContext } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import {
  decodeWsMessage,
  EventType,
  LogLevel,
  getLogLevelName,
  type LogEvent,
} from '@/api/proto/log_event';
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
  Bug,
  Info,
  AlertTriangle,
  Terminal,
  XCircle,
  Pause,
  Play,
  Trash2,
  Search,
  ArrowDown,
  Wifi,
  WifiOff,
} from 'lucide-react';
import { cn } from '@/lib/utils';

const MAX_LOG_ENTRIES = 500;
const WS_RECONNECT_BASE_DELAY = 1000;
const WS_RECONNECT_MAX_DELAY = 30000;

interface DisplayLogEvent extends LogEvent {
  id: number;
}

/** Build WebSocket URL for log streaming */
function buildLogWebSocketUrl(accessToken: string): string {
  const apiBaseUrl = import.meta.env.VITE_API_BASE_URL || '/api';

  let wsUrl: string;

  if (apiBaseUrl.startsWith('http://') || apiBaseUrl.startsWith('https://')) {
    const url = new URL(apiBaseUrl);
    const wsProtocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    wsUrl = `${wsProtocol}//${url.host}${url.pathname}`;
  } else {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    wsUrl = `${protocol}//${window.location.host}${apiBaseUrl}`;
  }

  const basePath = wsUrl.replace(/\/$/, '');
  return `${basePath}/logging/stream?token=${accessToken}`;
}

/** Get log level icon component */
function getLevelIcon(level: LogLevel) {
  const iconClass = 'w-3.5 h-3.5 shrink-0';
  switch (level) {
    case LogLevel.LOG_LEVEL_TRACE:
      return <Terminal className={cn(iconClass, 'text-slate-400')} />;
    case LogLevel.LOG_LEVEL_DEBUG:
      return <Bug className={cn(iconClass, 'text-blue-400')} />;
    case LogLevel.LOG_LEVEL_INFO:
      return <Info className={cn(iconClass, 'text-emerald-400')} />;
    case LogLevel.LOG_LEVEL_WARN:
      return <AlertTriangle className={cn(iconClass, 'text-amber-400')} />;
    case LogLevel.LOG_LEVEL_ERROR:
      return <XCircle className={cn(iconClass, 'text-rose-400')} />;
    default:
      return <Terminal className={cn(iconClass, 'text-muted-foreground')} />;
  }
}

/** Get log level background color classes */
function getLevelBgColor(level: LogLevel): string {
  switch (level) {
    case LogLevel.LOG_LEVEL_TRACE:
      return 'bg-slate-500/5 hover:bg-slate-500/10';
    case LogLevel.LOG_LEVEL_DEBUG:
      return 'bg-blue-500/5 hover:bg-blue-500/10';
    case LogLevel.LOG_LEVEL_INFO:
      return 'bg-emerald-500/5 hover:bg-emerald-500/10';
    case LogLevel.LOG_LEVEL_WARN:
      return 'bg-amber-500/5 hover:bg-amber-500/10';
    case LogLevel.LOG_LEVEL_ERROR:
      return 'bg-rose-500/5 hover:bg-rose-500/10';
    default:
      return 'hover:bg-muted/50';
  }
}

/** Get log level badge color classes */
function getLevelBadgeColor(level: LogLevel): string {
  switch (level) {
    case LogLevel.LOG_LEVEL_TRACE:
      return 'bg-slate-500/10 text-slate-400 border-slate-500/20';
    case LogLevel.LOG_LEVEL_DEBUG:
      return 'bg-blue-500/10 text-blue-400 border-blue-500/20';
    case LogLevel.LOG_LEVEL_INFO:
      return 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20';
    case LogLevel.LOG_LEVEL_WARN:
      return 'bg-amber-500/10 text-amber-400 border-amber-500/20';
    case LogLevel.LOG_LEVEL_ERROR:
      return 'bg-rose-500/10 text-rose-400 border-rose-500/20';
    default:
      return 'bg-muted/50 text-muted-foreground border-muted';
  }
}

type FilterLevel = 'all' | 'trace' | 'debug' | 'info' | 'warn' | 'error';

export function LogViewer() {
  const [logs, setLogs] = useState<DisplayLogEvent[]>([]);
  const [isPaused, setIsPaused] = useState(false);
  const [isConnected, setIsConnected] = useState(false);
  const [filterLevel, setFilterLevel] = useState<FilterLevel>('all');
  const [searchQuery, setSearchQuery] = useState('');
  const [autoScroll, setAutoScroll] = useState(true);

  const wsRef = useRef<WebSocket | null>(null);
  const reconnectAttemptRef = useRef(0);
  const reconnectTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(
    undefined,
  );
  const logContainerRef = useRef<HTMLDivElement>(null);
  const logIdRef = useRef(0);
  const pausedLogsRef = useRef<DisplayLogEvent[]>([]);

  const { user: routeUser } = useRouteContext({ from: '__root__' }) as {
    user?: any;
  };
  const { data: sessionData } = useQuery({
    ...sessionQueryOptions,
    enabled: typeof window !== 'undefined',
    initialData: routeUser ?? null,
  });
  const accessToken = sessionData?.token?.access_token;

  // Handle WebSocket message
  const handleMessage = useCallback(
    (event: MessageEvent) => {
      try {
        const data = new Uint8Array(event.data as ArrayBuffer);
        const message = decodeWsMessage(data);

        if (
          message.eventType === EventType.EVENT_TYPE_LOG &&
          'log' in message.payload
        ) {
          const logEvent: DisplayLogEvent = {
            ...message.payload.log,
            id: logIdRef.current++,
          };

          if (isPaused) {
            pausedLogsRef.current.push(logEvent);
            // Limit paused buffer too
            if (pausedLogsRef.current.length > MAX_LOG_ENTRIES) {
              pausedLogsRef.current =
                pausedLogsRef.current.slice(-MAX_LOG_ENTRIES);
            }
          } else {
            setLogs((prev) => {
              const newLogs = [...prev, logEvent];
              return newLogs.length > MAX_LOG_ENTRIES
                ? newLogs.slice(-MAX_LOG_ENTRIES)
                : newLogs;
            });
          }
        }
      } catch (error) {
        console.error('Failed to decode log message:', error);
      }
    },
    [isPaused],
  );

  // Connect to WebSocket
  const connect = useCallback(() => {
    if (!accessToken) return;
    if (wsRef.current?.readyState === WebSocket.OPEN) return;
    if (wsRef.current?.readyState === WebSocket.CONNECTING) return;

    const wsUrl = buildLogWebSocketUrl(accessToken);
    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';

    ws.onopen = () => {
      setIsConnected(true);
      reconnectAttemptRef.current = 0;
    };

    ws.onmessage = handleMessage;

    ws.onclose = () => {
      setIsConnected(false);
      wsRef.current = null;

      // Reconnect if we have a token
      if (accessToken) {
        const delay = Math.min(
          WS_RECONNECT_BASE_DELAY * Math.pow(2, reconnectAttemptRef.current),
          WS_RECONNECT_MAX_DELAY,
        );
        reconnectAttemptRef.current++;
        reconnectTimeoutRef.current = setTimeout(connect, delay);
      }
    };

    ws.onerror = () => {
      setIsConnected(false);
    };

    wsRef.current = ws;
  }, [accessToken, handleMessage]);

  // Disconnect
  const disconnect = useCallback(() => {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
    }
    if (wsRef.current) {
      wsRef.current.close();
      wsRef.current = null;
    }
  }, []);

  // Connection lifecycle
  useEffect(() => {
    if (accessToken) {
      connect();
    }
    return () => disconnect();
  }, [accessToken, connect, disconnect]);

  // Auto scroll to bottom
  useEffect(() => {
    if (autoScroll && logContainerRef.current && !isPaused) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight;
    }
  }, [logs, autoScroll, isPaused]);

  // Handle pause/resume
  const togglePause = useCallback(() => {
    if (isPaused) {
      // Resume: add paused logs
      setLogs((prev) => {
        const combined = [...prev, ...pausedLogsRef.current];
        pausedLogsRef.current = [];
        return combined.length > MAX_LOG_ENTRIES
          ? combined.slice(-MAX_LOG_ENTRIES)
          : combined;
      });
    }
    setIsPaused(!isPaused);
  }, [isPaused]);

  // Clear logs
  const clearLogs = useCallback(() => {
    setLogs([]);
    pausedLogsRef.current = [];
  }, []);

  // Filter logs - memoized to avoid recalculating on every render
  const filteredLogs = useMemo(() => {
    return logs.filter((log) => {
      // Level filter
      if (filterLevel !== 'all') {
        const levelMap: Record<FilterLevel, LogLevel> = {
          all: LogLevel.LOG_LEVEL_UNSPECIFIED,
          trace: LogLevel.LOG_LEVEL_TRACE,
          debug: LogLevel.LOG_LEVEL_DEBUG,
          info: LogLevel.LOG_LEVEL_INFO,
          warn: LogLevel.LOG_LEVEL_WARN,
          error: LogLevel.LOG_LEVEL_ERROR,
        };
        if (log.level < levelMap[filterLevel]) return false;
      }

      // Search filter
      if (searchQuery) {
        const query = searchQuery.toLowerCase();
        return (
          log.target.toLowerCase().includes(query) ||
          log.message.toLowerCase().includes(query)
        );
      }

      return true;
    });
  }, [logs, filterLevel, searchQuery]);

  // Format timestamp - memoized function
  const formatTime = useCallback((timestampMs: bigint) => {
    const date = new Date(Number(timestampMs));
    return date.toLocaleTimeString('en-US', {
      hour12: false,
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      fractionalSecondDigits: 3,
    });
  }, []);

  return (
    <Card className="border-border/40 bg-gradient-to-b from-card to-card/80 shadow-lg">
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
                <Trans>
                  View application logs in real-time. Logs are limited to the
                  last {MAX_LOG_ENTRIES} entries.
                </Trans>
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
              <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
              <Input
                placeholder={t`Search logs...`}
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="pl-9"
              />
            </div>
            <Select
              value={filterLevel}
              onValueChange={(v) => setFilterLevel(v as FilterLevel)}
            >
              <SelectTrigger className="w-full sm:w-[140px]">
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
                autoScroll ? t`Auto-scroll enabled` : t`Auto-scroll disabled`
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
          className="h-[400px] overflow-y-auto rounded-lg border border-border/40 bg-black/20 font-mono text-xs"
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
              <motion.div
                key={log.id}
                initial={{ opacity: 0, x: -10 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ duration: 0.1 }}
                className={cn(
                  'flex items-start gap-2 px-3 py-1.5 border-b border-border/20 transition-colors',
                  getLevelBgColor(log.level),
                )}
              >
                <span className="text-muted-foreground shrink-0 w-[85px]">
                  {formatTime(log.timestampMs)}
                </span>
                <Badge
                  variant="outline"
                  className={cn(
                    'text-[9px] uppercase font-medium shrink-0 px-1.5 py-0',
                    getLevelBadgeColor(log.level),
                  )}
                >
                  {getLevelIcon(log.level)}
                  <span className="ml-1">{getLogLevelName(log.level)}</span>
                </Badge>
                <span className="text-primary/80 shrink-0 max-w-[150px] truncate">
                  {log.target}
                </span>
                <span className="text-foreground/90 break-all flex-1">
                  {log.message}
                </span>
              </motion.div>
            ))
          )}
        </div>

        {/* Status bar */}
        <div className="flex items-center justify-between mt-3 text-xs text-muted-foreground">
          <span>
            {filteredLogs.length} / {logs.length} <Trans>entries</Trans>
            {isPaused && pausedLogsRef.current.length > 0 && (
              <span className="ml-2 text-amber-400">
                (+{pausedLogsRef.current.length} <Trans>paused</Trans>)
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
