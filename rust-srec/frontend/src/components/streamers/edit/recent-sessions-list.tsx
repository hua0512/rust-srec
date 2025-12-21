import { FileVideo, Calendar, Clock, HardDrive } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';
import { useLingui } from '@lingui/react';

interface RecentSessionsListProps {
  sessions: any[];
  isLoading: boolean;
}

export function RecentSessionsList({
  sessions,
  isLoading,
}: RecentSessionsListProps) {
  const { i18n } = useLingui();
  const recentSessions = sessions
    ? [...sessions]
        .sort(
          (a: any, b: any) =>
            new Date(b.start_time).getTime() - new Date(a.start_time).getTime(),
        )
        .slice(0, 5)
    : [];

  return (
    <div className="p-6 rounded-xl border bg-card/50 shadow-sm space-y-4">
      <div className="flex items-center gap-2 font-semibold text-lg text-primary">
        <FileVideo className="w-5 h-5" /> <Trans>Recent Sessions</Trans>
      </div>

      {isLoading ? (
        <div className="space-y-3">
          {[1, 2, 3].map((i) => (
            <div
              key={i}
              className="h-16 bg-muted/20 animate-pulse rounded-lg"
            />
          ))}
        </div>
      ) : recentSessions.length > 0 ? (
        <div className="space-y-3">
          {recentSessions.map((session) => (
            <div
              key={session.id}
              className="group flex flex-col gap-1 p-3 rounded-lg border bg-background/50 hover:bg-background hover:border-primary/20 transition-all"
            >
              <div className="flex items-center justify-between">
                <span
                  className="text-xs font-medium truncate max-w-[120px]"
                  title={session.title}
                >
                  {session.title}
                </span>
                <span
                  className={cn(
                    'text-[10px] px-1.5 py-0.5 rounded-full border',
                    session.end_time
                      ? 'bg-muted/30 text-muted-foreground border-transparent'
                      : 'bg-emerald-500/10 text-emerald-500 border-emerald-500/20',
                  )}
                >
                  {session.end_time ? (
                    <Trans>Offline</Trans>
                  ) : (
                    <Trans>Live</Trans>
                  )}
                </span>
              </div>
              <div className="flex items-center gap-3 text-[10px] text-muted-foreground">
                <div className="flex items-center gap-1">
                  <Calendar className="w-3 h-3" />
                  <span>
                    {i18n.date(session.start_time, {
                      month: 'short',
                      day: 'numeric',
                    })}
                  </span>
                </div>
                <div className="flex items-center gap-1">
                  <Clock className="w-3 h-3" />
                  <span>
                    {session.duration_secs
                      ? `${Math.floor(session.duration_secs / 60)}m`
                      : '-'}
                  </span>
                </div>
                <div className="flex items-center gap-1 ml-auto">
                  <HardDrive className="w-3 h-3" />
                  <span>
                    {session.total_size_bytes
                      ? (session.total_size_bytes / 1024 / 1024).toFixed(1) +
                        ' MB'
                      : '-'}
                  </span>
                </div>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="text-sm text-muted-foreground py-4 text-center">
          <Trans>No recent sessions found.</Trans>
        </div>
      )}
    </div>
  );
}
