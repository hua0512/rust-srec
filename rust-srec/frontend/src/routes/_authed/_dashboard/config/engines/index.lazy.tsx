import { createLazyFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listEngines } from '@/server/functions';
import {
  EngineCard,
  CreateEngineCard,
} from '@/components/config/engines/engine-card';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Skeleton } from '@/components/ui/skeleton';
import { CardSkeleton } from '@/components/shared/card-skeleton';
import { AlertCircle } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { containerVariants, itemVariants } from '@/lib/animation';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/engines/')(
  {
    component: EnginesConfigPage,
  },
);

function EnginesConfigPage() {
  const {
    data: engines,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>
          <Trans>Error</Trans>
        </AlertTitle>
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
          initial={false}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0, transition: { duration: 0.1 } }}
          className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
        >
          {[1, 2, 3, 4].map((i) => (
            <CardSkeleton key={i}>
              <div className="flex items-start gap-3">
                <Skeleton className="h-10 w-10 rounded-lg" />
                <div className="flex-1 space-y-2">
                  <Skeleton className="h-4 w-2/3" />
                  <Skeleton className="h-3 w-1/3" />
                </div>
                <Skeleton className="h-6 w-20 rounded-full" />
              </div>
              <div className="space-y-2 pt-4">
                <Skeleton className="h-3 w-16" />
                <Skeleton className="h-9 w-full rounded-md" />
              </div>
              <div className="mt-auto flex justify-end gap-2 pt-4">
                <Skeleton className="h-9 w-9 rounded-md" />
                <Skeleton className="h-9 w-28 rounded-md" />
              </div>
            </CardSkeleton>
          ))}
        </motion.div>
      ) : (
        <motion.div
          key="list"
          className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
          variants={containerVariants}
          initial="hidden"
          animate="visible"
          exit="exit"
        >
          {engines?.map((engine) => (
            <motion.div
              key={engine.id}
              variants={itemVariants}
              className="h-full"
            >
              <EngineCard engine={engine} />
            </motion.div>
          ))}
          <motion.div variants={itemVariants} className="h-full">
            <CreateEngineCard />
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
