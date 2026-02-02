import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listPlatformConfigs } from '@/server/functions';
import { Skeleton } from '@/components/ui/skeleton';
import { PlatformCard } from '@/components/config/platforms/platform-card';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { AlertCircle, Globe, Search } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { useState, useEffect, useMemo } from 'react';

export const Route = createFileRoute('/_authed/_dashboard/config/platforms/')({
  component: PlatformsConfigPage,
});

function PlatformsConfigPage() {
  const navigate = useNavigate();
  const { i18n } = useLingui();
  const [searchQuery, setSearchQuery] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(searchQuery);
    }, 300);
    return () => clearTimeout(timer);
  }, [searchQuery]);

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
    const search = debouncedSearch.toLowerCase();
    return platforms.filter((p) => p.name.toLowerCase().includes(search));
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
        <div className="relative flex-1 max-w-sm">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            placeholder={i18n._(msg`Search platforms...`)}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="pl-9 h-9 bg-muted/50 border-muted-foreground/20 focus:bg-background transition-colors"
          />
        </div>
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
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4 gap-6"
          >
            {[1, 2, 3, 4].map((i) => (
              <div
                key={i}
                className="h-[220px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm"
              >
                <div className="flex justify-between items-start">
                  <Skeleton className="h-10 w-10 rounded-full" />
                  <Skeleton className="h-6 w-16" />
                </div>
                <div className="space-y-2 pt-2">
                  <Skeleton className="h-6 w-3/4" />
                  <Skeleton className="h-4 w-1/2" />
                </div>
                <div className="pt-4 mt-auto">
                  <Skeleton className="h-16 w-full rounded-md" />
                </div>
              </div>
            ))}
          </motion.div>
        ) : filteredPlatforms.length > 0 ? (
          <motion.div
            key="list"
            className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4 gap-6"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: 0.3 }}
          >
            {filteredPlatforms.map((platform, index) => (
              <motion.div
                key={platform.id}
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{
                  duration: 0.3,
                  delay: Math.min(index * 0.05, 0.3),
                }}
              >
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
