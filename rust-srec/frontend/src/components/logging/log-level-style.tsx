import {
  AlertTriangle,
  Bug,
  Eye,
  Info,
  Terminal,
  XCircle,
  type LucideIcon,
} from 'lucide-react';
import { LogLevel as ProtoLogLevel } from '@/api/proto/gen/log_event_pb.js';
import type { LogLevel } from '@/api/schemas/logging';
import { cn } from '@/lib/utils';

interface LogLevelStyle {
  icon: LucideIcon;
  iconClassName: string;
  /** A level badge's colours. */
  badgeClassName: string;
  /** A streamed log row's tint. */
  rowClassName: string;
}

const UNKNOWN_LEVEL_STYLE: LogLevelStyle = {
  icon: Terminal,
  iconClassName: 'text-muted-foreground',
  badgeClassName: 'bg-muted/50 text-muted-foreground border-muted',
  rowClassName: 'hover:bg-muted/50',
};

/** How each log level looks wherever it is shown. */
export const LOG_LEVEL_STYLE: Record<LogLevel, LogLevelStyle> = {
  trace: {
    icon: Terminal,
    iconClassName: 'text-slate-400',
    badgeClassName: 'bg-slate-500/10 text-slate-400 border-slate-500/20',
    rowClassName: 'bg-slate-500/5 hover:bg-slate-500/10',
  },
  debug: {
    icon: Bug,
    iconClassName: 'text-blue-400',
    badgeClassName: 'bg-blue-500/10 text-blue-400 border-blue-500/20',
    rowClassName: 'bg-blue-500/5 hover:bg-blue-500/10',
  },
  info: {
    icon: Info,
    iconClassName: 'text-emerald-400',
    badgeClassName: 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20',
    rowClassName: 'bg-emerald-500/5 hover:bg-emerald-500/10',
  },
  warn: {
    icon: AlertTriangle,
    iconClassName: 'text-amber-400',
    badgeClassName: 'bg-amber-500/10 text-amber-400 border-amber-500/20',
    rowClassName: 'bg-amber-500/5 hover:bg-amber-500/10',
  },
  error: {
    icon: XCircle,
    iconClassName: 'text-rose-400',
    badgeClassName: 'bg-rose-500/10 text-rose-400 border-rose-500/20',
    rowClassName: 'bg-rose-500/5 hover:bg-rose-500/10',
  },
  off: {
    ...UNKNOWN_LEVEL_STYLE,
    icon: Eye,
    iconClassName: 'text-muted-foreground/50',
  },
};

const PROTO_LEVELS: Partial<Record<ProtoLogLevel, LogLevel>> = {
  [ProtoLogLevel.TRACE]: 'trace',
  [ProtoLogLevel.DEBUG]: 'debug',
  [ProtoLogLevel.INFO]: 'info',
  [ProtoLogLevel.WARN]: 'warn',
  [ProtoLogLevel.ERROR]: 'error',
};

/** The level of a streamed log event, or `undefined` for one it does not name. */
export function fromProtoLogLevel(level: ProtoLogLevel): LogLevel | undefined {
  return PROTO_LEVELS[level];
}

/** The style of `level`, or a neutral one for an unnamed level. */
export function logLevelStyle(level: LogLevel | undefined): LogLevelStyle {
  return level ? LOG_LEVEL_STYLE[level] : UNKNOWN_LEVEL_STYLE;
}

export function LogLevelIcon({ level }: { level: LogLevel | undefined }) {
  const { icon: Icon, iconClassName } = logLevelStyle(level);
  return <Icon className={cn('size-3.5 shrink-0', iconClassName)} />;
}
