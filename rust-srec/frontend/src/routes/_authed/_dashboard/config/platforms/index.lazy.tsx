import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listPlatformConfigs } from '@/server/functions';
import { CardSkeleton } from '@/components/shared/card-skeleton';
import { PlatformCard } from '@/components/config/platforms/platform-card';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { SearchInput } from '@/components/shared/search-input';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { AlertCircle, Globe } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { useMemo } from 'react';
import { containerVariants, itemVariants } from '@/lib/animation';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/config/platforms/',
)({
  component: PlatformsConfigPage,
});

function PlatformsConfigPage() {
  const navigate = useNavigate();
  const { i18n } = useLingui();
  const search = Route.useSearch();
  const updateSearch = useUpdateSearch<typeof search>();
  const debouncedSearch = search.q ?? '';

  const {
    data: platforms,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['config', 'platforms'],
    queryFn: () => listPlatformConfigs(),
  });

  // Filter platforms by search
  const filteredPlatforms = useMemo(() => {
    if (!platforms) return [];
    if (!debouncedSearch) return platforms;
    const term = debouncedSearch.toLowerCase();
    return platforms.filter((p) => p.name.toLowerCase().includes(term));
  }, [platforms, debouncedSearch]);

  const handleEdit = (platformId: string) => {
    void navigate({
      to: '/config/platforms/$platformId',
      params: { platformId },
    });
  };

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>
          <Trans>Error</Trans>
        </AlertTitle>
        <AlertDescription>
          <Trans>Failed to load platforms: {(error as Error).message}</Trans>
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <div className="space-y-6">
      {/* Search Bar */}
      <div className="flex items-center gap-4 px-1">
        <SearchInput
          defaultValue={debouncedSearch}
          onSearch={(value) => updateSearch({ q: value || undefined })}
          placeholder={i18n._(msg`Search platforms...`)}
          className="flex-1 max-w-sm"
        />
        <Badge
          variant="secondary"
          className="h-7 px-3 text-sm whitespace-nowrap bg-muted/50 text-muted-foreground border-border/50"
        >
          {filteredPlatforms.length} <Trans>platforms</Trans>
        </Badge>
      </div>

      <AnimatePresence mode="wait">
        {isLoading ? (
          <motion.div
            key="loading"
            initial={false}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0, transition: { duration: 0.1 } }}
            className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4 gap-6"
          >
            {[1, 2, 3, 4].map((i) => (
              <CardSkeleton key={i} />
            ))}
          </motion.div>
        ) : filteredPlatforms.length > 0 ? (
          <motion.div
            key="list"
            className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4 gap-6"
            variants={containerVariants}
            initial="hidden"
            animate="visible"
            exit="exit"
          >
            {filteredPlatforms.map((platform) => (
              <motion.div key={platform.id} variants={itemVariants}>
                <PlatformCard
                  platform={platform}
                  onEdit={() => handleEdit(platform.id)}
                />
              </motion.div>
            ))}
          </motion.div>
        ) : (
          <motion.div
            key="empty"
            initial={{ opacity: 0, scale: 0.95 }}
            animate={{ opacity: 1, scale: 1 }}
            className="flex flex-col items-center justify-center py-32 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5 backdrop-blur-sm shadow-sm"
          >
            <div className="p-6 bg-primary/5 rounded-full ring-1 ring-primary/10">
              <Globe className="h-16 w-16 text-primary/60" />
            </div>
            <div className="space-y-2 max-w-md">
              <h3 className="font-semibold text-2xl tracking-tight">
                {debouncedSearch ? (
                  <Trans>No platforms found</Trans>
                ) : (
                  <Trans>No platforms configured</Trans>
                )}
              </h3>
              <p className="text-muted-foreground">
                {debouncedSearch ? (
                  <Trans>Try adjusting your search.</Trans>
                ) : (
                  <Trans>
                    Platform configurations will appear here when available.
                  </Trans>
                )}
              </p>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
