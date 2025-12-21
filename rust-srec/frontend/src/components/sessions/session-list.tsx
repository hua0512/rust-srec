import { SessionCard } from './session-card';
import { Skeleton } from '../ui/skeleton';
import { SessionSchema } from '../../api/schemas';
import { z } from 'zod';
import { Button } from '../ui/button';
import { RefreshCcw } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

type Session = z.infer<typeof SessionSchema>;

interface SessionListProps {
  sessions: Session[];
  isLoading: boolean;
  onRefresh?: () => void;
  token?: string;
}

export function SessionList({
  sessions,
  isLoading,
  onRefresh,
  token,
}: SessionListProps) {
  if (isLoading) {
    return (
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-4">
        {Array.from({ length: 8 }).map((_, i) => (
          <div
            key={i}
            className="flex flex-col h-full bg-white/60 dark:bg-card/40 backdrop-blur-xl border-black/5 dark:border-white/5 rounded-xl overflow-hidden shadow-sm dark:shadow-xl"
          >
            <div className="p-4 pb-2 flex flex-row gap-3 items-center">
              <Skeleton className="h-10 w-10 rounded-full" />
              <div className="flex-1 space-y-1.5">
                <Skeleton className="h-3 w-20" />
                <Skeleton className="h-4 w-3/4" />
              </div>
            </div>
            <div className="p-4 pt-2 grow">
              <Skeleton className="w-full aspect-video rounded-md mb-4" />
              <div className="grid grid-cols-2 gap-3">
                <Skeleton className="h-3 w-24" />
                <Skeleton className="h-3 w-20" />
                <Skeleton className="h-3 w-24" />
                <Skeleton className="h-3 w-16" />
              </div>
            </div>
            <div className="p-2 px-4 border-t bg-muted/20 flex justify-between items-center py-3">
              <Skeleton className="h-3 w-20" />
              <Skeleton className="h-8 w-8 rounded-md" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (sessions.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center p-12 text-center border-dashed rounded-xl bg-white/60 dark:bg-card/30 backdrop-blur-xl border-black/5 dark:border-white/5 shadow-sm">
        <div className="bg-muted/50 p-4 rounded-full mb-4">
          <RefreshCcw className="h-8 w-8 text-muted-foreground/50" />
        </div>
        <h3 className="text-lg font-semibold text-foreground">
          <Trans>No sessions found</Trans>
        </h3>
        <p className="text-muted-foreground text-sm max-w-sm mt-1 mb-4">
          <Trans>There are no recorded sessions matching your criteria.</Trans>
        </p>
        {onRefresh && (
          <Button variant="outline" onClick={onRefresh}>
            <Trans>Refresh</Trans>
          </Button>
        )}
      </div>
    );
  }

  return (
    <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-4 animate-in fade-in duration-500">
      {sessions.map((session) => (
        <SessionCard key={session.id} session={session} token={token} />
      ))}
    </div>
  );
}
