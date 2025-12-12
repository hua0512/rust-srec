import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listEngines } from '@/server/functions';
import { EngineCard, CreateEngineCard } from '@/components/config/engines/engine-card';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Skeleton } from '@/components/ui/skeleton';
import { AlertCircle, Cpu } from 'lucide-react';
import { Trans } from "@lingui/react/macro";

export const Route = createFileRoute('/_authed/_dashboard/config/engines')({
  component: EnginesPage,
});

function EnginesPage() {
  const { data: engines, isLoading, error } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle><Trans>Error</Trans></AlertTitle>
        <AlertDescription>
          <Trans>Failed to load engines: {error.message}</Trans>
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <AnimatePresence mode="wait">
      {isLoading ? (
        <motion.div
          key="loading"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
        >
          {[1, 2, 3, 4].map((i) => (
            <div key={i} className="h-[200px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm">
              <div className="flex justify-between items-start">
                <Skeleton className="h-12 w-12 rounded-xl" />
                <Skeleton className="h-6 w-16" />
              </div>
              <div className="space-y-2 pt-2">
                <Skeleton className="h-5 w-3/4" />
                <Skeleton className="h-4 w-1/2" />
              </div>
              <div className="pt-4 mt-auto">
                <Skeleton className="h-8 w-full rounded-md" />
              </div>
            </div>
          ))}
        </motion.div>
      ) : (
        <motion.div
          key="list"
          className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.3 }}
        >
          {engines?.map((engine, index) => (
            <motion.div
              key={engine.id}
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{
                duration: 0.3,
                delay: Math.min(index * 0.05, 0.3)
              }}
            >
              <EngineCard engine={engine} />
            </motion.div>
          ))}
          <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{
              duration: 0.3,
              delay: Math.min((engines?.length || 0) * 0.05, 0.3)
            }}
          >
            <CreateEngineCard />
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
