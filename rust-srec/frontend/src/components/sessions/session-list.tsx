import { motion, AnimatePresence } from 'motion/react';
import { SessionCard } from './session-card';
import { Skeleton } from '../ui/skeleton';
import { SessionSchema } from '../../api/schemas';
import { z } from 'zod';
import { Button } from '../ui/button';
import { RefreshCcw, LayoutGrid } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

type Session = z.infer<typeof SessionSchema>;

interface SessionListProps {
  sessions: Session[];
  isLoading: boolean;
  onRefresh?: () => void;
  token?: string;
  selectionMode?: boolean;
  selectedIds?: Set<string>;
  onSelectionChange?: (id: string, selected: boolean) => void;
}

export function SessionList({
  sessions,
  isLoading,
  onRefresh,
  token,
  selectionMode,
  selectedIds,
  onSelectionChange,
}: SessionListProps) {
  if (isLoading) {
    return (
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-6">
        {Array.from({ length: 10 }).map((_, i) => (
          <div
            key={i}
            className="flex flex-col h-full bg-card/10 backdrop-blur-3xl border-white/5 rounded-2xl overflow-hidden shadow-2xl space-y-4 p-5"
          >
            <div className="flex flex-row gap-4 items-center">
              <Skeleton className="h-12 w-12 rounded-2xl bg-white/5" />
              <div className="flex-1 space-y-2.5">
                <Skeleton className="h-3 w-20 bg-white/5" />
                <Skeleton className="h-4 w-3/4 bg-white/5" />
              </div>
            </div>
            <div className="space-y-3">
              <Skeleton className="h-10 w-full rounded-xl bg-white/5" />
              <div className="grid grid-cols-2 gap-3">
                <Skeleton className="h-14 w-full rounded-xl bg-white/5" />
                <Skeleton className="h-14 w-full rounded-xl bg-white/5" />
              </div>
            </div>
            <div className="pt-2 flex justify-between items-center">
              <Skeleton className="h-9 w-32 rounded-xl bg-white/5" />
              <Skeleton className="h-9 w-9 rounded-xl bg-white/5" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (sessions.length === 0) {
    return (
      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        className="flex flex-col items-center justify-center p-20 text-center rounded-[2rem] bg-card/10 backdrop-blur-3xl border border-white/5 shadow-2xl relative overflow-hidden group"
      >
        <div className="absolute inset-0 bg-gradient-to-br from-primary/5 via-transparent to-transparent opacity-50" />
        <div className="bg-gradient-to-br from-white/10 to-transparent p-8 rounded-[2rem] mb-6 shadow-2xl border border-white/10 group-hover:scale-110 transition-transform duration-700">
          <LayoutGrid className="h-12 w-12 text-primary/40 group-hover:text-primary transition-colors duration-700" />
        </div>
        <h3 className="text-2xl font-black text-foreground/90 tracking-tight">
          <Trans>No Archives Found</Trans>
        </h3>
        <p className="text-muted-foreground/60 text-sm max-w-sm mt-3 mb-8 font-medium leading-relaxed">
          <Trans>
            Your digital library is currently empty. Start recording to populate
            your sessions here.
          </Trans>
        </p>
        {onRefresh && (
          <Button
            variant="ghost"
            onClick={onRefresh}
            className="rounded-2xl px-8 h-12 font-black tracking-widest text-xs border border-white/10 hover:bg-white/10 hover:border-primary/30 transition-all gap-3"
          >
            <RefreshCcw className="h-4 w-4" />
            <Trans>REFRESH LIBRARY</Trans>
          </Button>
        )}
      </motion.div>
    );
  }

  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4">
      <AnimatePresence mode="popLayout">
        {sessions.map((session, idx) => (
          <motion.div
            key={session.id}
            layout
            initial={{ opacity: 0, scale: 0.9, y: 20 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.9, transition: { duration: 0.2 } }}
            transition={{
              type: 'spring',
              stiffness: 260,
              damping: 20,
              delay: Math.min(idx * 0.04, 0.4),
            }}
          >
            <SessionCard
              session={session}
              token={token}
              selectionMode={selectionMode}
              isSelected={selectedIds?.has(session.id)}
              onSelectChange={onSelectionChange}
            />
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
