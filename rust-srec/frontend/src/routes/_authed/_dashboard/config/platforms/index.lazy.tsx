import { createLazyFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listPlatformConfigs } from '@/server/functions';
import { CardSkeleton } from '@/components/shared/card-skeleton';
import { PlatformCard } from '@/components/config/platforms/platform-card';
import { Badge } from '@/components/ui/badge';
import { SearchInput } from '@/components/shared/search-input';
import { PageEmptyState } from '@/components/shared/list-panel';
import { QueryError } from '@/components/shared/query-error';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { Globe } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { useMemo } from 'react';
import { containerVariants, itemVariants } from '@/lib/animation';
import { COMPACT_CARD_GRID } from '@/lib/grid';
import { cn } from '@/lib/utils';

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
    refetch,
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
        {error && !platforms ? (
          <QueryError
            key="error"
            title={<Trans>Failed to load platforms</Trans>}
            error={error}
            onRetry={() => void refetch()}
          />
        ) : isLoading ? (
          <motion.div
            key="loading"
            initial={false}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0, transition: { duration: 0.1 } }}
            className={cn(COMPACT_CARD_GRID, 'gap-6')}
          >
            {[1, 2, 3, 4].map((i) => (
              <CardSkeleton key={i} />
            ))}
          </motion.div>
        ) : filteredPlatforms.length > 0 ? (
          <motion.div
            key="list"
            className={cn(COMPACT_CARD_GRID, 'gap-6')}
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
          <PageEmptyState
            key="empty"
            icon={Globe}
            title={
              debouncedSearch ? (
                <Trans>No platforms found</Trans>
              ) : (
                <Trans>No platforms configured</Trans>
              )
            }
            help={
              debouncedSearch ? (
                <Trans>Try adjusting your search.</Trans>
              ) : (
                <Trans>
                  Platform configurations will appear here when available.
                </Trans>
              )
            }
          />
        )}
      </AnimatePresence>
    </div>
  );
}
