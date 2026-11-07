import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { z } from 'zod';
import { useState, useMemo, useRef, useCallback } from 'react';
import { queryKeys } from '@/api/query-keys';
import {
  listJobPresets,
  deleteJobPreset,
  cloneJobPreset,
} from '@/server/functions/job';
import { JobPresetSchema } from '@/api/schemas';

import { Button } from '@/components/ui/button';
import { Plus, Settings2 } from 'lucide-react';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { PresetCard } from '@/components/pipeline/presets/preset-card';
import {
  StepInUseDialog,
  stepUsersFromConflict,
  type StepInUse,
} from '@/components/pipeline/step-in-use-dialog';
import { getCategoryName } from '@/components/pipeline/presets/default-presets-i18n';
import { SearchInput } from '@/components/shared/search-input';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { Skeleton } from '@/components/ui/skeleton';
import { CardSkeleton } from '@/components/shared/card-skeleton';
import { containerVariants, itemVariants } from '@/lib/animation';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Badge } from '@/components/ui/badge';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { ListPagination } from '@/components/shared/list-pagination';
import { PageEmptyState } from '@/components/shared/list-panel';
import { CreateFab } from '@/components/shared/page-actions';
import { QueryError } from '@/components/shared/query-error';
import { CARD_GRID } from '@/lib/grid';
import { cn } from '@/lib/utils';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/presets/',
)({
  component: PresetsPage,
});

function PresetsPage() {
  const { i18n } = useLingui();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();

  // Category, committed search, and pagination live in the URL so they persist
  // across navigation into presets/$presetId and page reloads.
  const selectedCategory = search.category ?? null;
  const debouncedSearch = search.q ?? '';
  const pageSize = search.size ?? 24;
  // The URL counts pages from 1; the pager counts them from 0.
  const currentPage = (search.page ?? 1) - 1;

  const [cloneDialogOpen, setCloneDialogOpen] = useState(false);
  const [presetToClone, setPresetToClone] = useState<z.infer<
    typeof JobPresetSchema
  > | null>(null);
  const [cloneName, setCloneName] = useState('');
  const [inUse, setInUse] = useState<StepInUse>();

  const updateSearch = useUpdateSearch<typeof search>();

  const handleCategoryChange = useCallback(
    (category: string | null) => {
      updateSearch({ category: category ?? undefined, page: undefined });
    },
    [updateSearch],
  );

  // Server-side filtered query with pagination
  const { data, isLoading, isError, error, refetch } = useQuery({
    queryKey: queryKeys.jobPresets.list({
      category: selectedCategory,
      search: debouncedSearch,
      pageSize,
      page: currentPage,
    }),
    queryFn: () =>
      listJobPresets({
        data: {
          category: selectedCategory || undefined,
          search: debouncedSearch || undefined,
          limit: pageSize,
          offset: currentPage * pageSize,
        },
      }),
    staleTime: 30000, // 30 seconds
    placeholderData: keepPreviousData,
  });

  const presets = data?.presets;
  const totalCount = data?.total ?? 0;

  // Memoize categories to prevent flickering
  const categoriesRef = useRef<string[]>([]);
  const categories = useMemo(() => {
    if (data?.categories && data.categories.length > 0) {
      categoriesRef.current = data.categories;
    }
    return categoriesRef.current;
  }, [data?.categories]);

  const handlePageIndexChange = useCallback(
    (index: number) =>
      updateSearch({ page: index > 0 ? index + 1 : undefined }),
    [updateSearch],
  );

  const deleteMutation = useMutation({
    mutationFn: deleteJobPreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Preset deleted successfully`));
      void queryClient.invalidateQueries({
        queryKey: queryKeys.jobPresets.all,
      });
    },
    onError: (error, { data: id }) => {
      const users = stepUsersFromConflict(error);
      if (!users) {
        toast.error(i18n._(msg`Failed to delete preset: ${error.message}`));
        return;
      }
      const name = presets?.find((preset) => preset.id === id)?.name ?? id;
      setInUse({ change: 'delete', step: 'preset', name, users });
    },
  });

  const cloneMutation = useMutation({
    mutationFn: cloneJobPreset,
    onSuccess: (cloned) => {
      toast.success(i18n._(msg`Preset cloned successfully`));
      void queryClient.invalidateQueries({
        queryKey: queryKeys.jobPresets.all,
      });
      setCloneDialogOpen(false);
      setPresetToClone(null);
      setCloneName('');
      // Navigate to edit the cloned preset
      void navigate({
        to: '/pipeline/presets/$presetId',
        params: { presetId: cloned.id },
      });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to clone preset: ${error.message}`)),
  });

  const handleDelete = (id: string) => {
    deleteMutation.mutate({ data: id });
  };

  const handleEdit = (preset: z.infer<typeof JobPresetSchema>) => {
    void navigate({
      to: '/pipeline/presets/$presetId',
      params: { presetId: preset.id },
    });
  };

  const handleClone = (preset: z.infer<typeof JobPresetSchema>) => {
    setPresetToClone(preset);
    setCloneName(`${preset.name}_copy`);
    setCloneDialogOpen(true);
  };

  const handleCloneConfirm = () => {
    if (presetToClone && cloneName.trim()) {
      cloneMutation.mutate({
        data: { id: presetToClone.id, new_name: cloneName.trim() },
      });
    }
  };

  const handleCreate = () => {
    void navigate({ to: '/pipeline/presets/create' });
  };

  return (
    <div className="min-h-screen space-y-6">
      {/* Header */}
      <DashboardHeader
        icon={Settings2}
        title={<Trans>Presets</Trans>}
        subtitle={
          <Trans>Reusable processor configurations for pipeline steps</Trans>
        }
        actions={
          <>
            <SearchInput
              defaultValue={debouncedSearch}
              onSearch={(value) =>
                updateSearch({ q: value || undefined, page: undefined })
              }
              placeholder={i18n._(msg`Search presets...`)}
              className="flex-1 md:w-64 min-w-[200px]"
            />
            <Badge
              variant="secondary"
              className="h-9 px-3 text-sm whitespace-nowrap"
            >
              {totalCount} <Trans>presets</Trans>
            </Badge>
          </>
        }
      >
        <nav className="flex items-center gap-1">
          <button
            onClick={() => handleCategoryChange(null)}
            className={`relative shrink-0 whitespace-nowrap px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 ${
              selectedCategory === null
                ? 'bg-primary text-primary-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground hover:bg-muted'
            }`}
          >
            <span className="relative z-10 flex items-center gap-1.5">
              <Trans>All</Trans>
            </span>
          </button>

          {categories.map((cat) => (
            <button
              key={cat}
              onClick={() => handleCategoryChange(cat)}
              className={`relative shrink-0 whitespace-nowrap px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 ${
                selectedCategory === cat
                  ? 'bg-primary text-primary-foreground shadow-sm'
                  : 'text-muted-foreground hover:text-foreground hover:bg-muted'
              }`}
            >
              <span className="relative z-10 capitalize">
                {getCategoryName(cat, i18n)}
              </span>
            </button>
          ))}
        </nav>
      </DashboardHeader>

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isError && !data ? (
            <QueryError
              key="error"
              title={<Trans>Failed to load presets</Trans>}
              error={error}
              onRetry={() => void refetch()}
            />
          ) : isLoading ? (
            <motion.div
              key="loading"
              initial={false}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className={cn(CARD_GRID, 'gap-6')}
            >
              {[1, 2, 3, 4].map((i) => (
                <CardSkeleton key={i}>
                  <div className="flex justify-between items-start">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <Skeleton className="h-6 w-8" />
                  </div>
                  <div className="space-y-2 pt-2">
                    <Skeleton className="h-6 w-3/4" />
                    <Skeleton className="h-4 w-1/2" />
                  </div>
                  <div className="pt-4 mt-auto">
                    <Skeleton className="h-20 w-full rounded-md" />
                  </div>
                </CardSkeleton>
              ))}
            </motion.div>
          ) : presets && presets.length > 0 ? (
            <motion.div
              key="list"
              className={cn(CARD_GRID, 'gap-6')}
              variants={containerVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              {presets.map((preset) => (
                <motion.div key={preset.id} variants={itemVariants}>
                  <PresetCard
                    preset={preset}
                    onEdit={handleEdit}
                    onDelete={handleDelete}
                    onClone={handleClone}
                  />
                </motion.div>
              ))}
            </motion.div>
          ) : (
            <PageEmptyState
              key="empty"
              icon={Settings2}
              title={
                debouncedSearch ? (
                  <Trans>No presets found</Trans>
                ) : (
                  <Trans>No presets yet</Trans>
                )
              }
              help={
                debouncedSearch ? (
                  <Trans>Try adjusting your search or filters.</Trans>
                ) : (
                  <Trans>
                    Create your first job preset to define reusable processor
                    configurations for your pipelines.
                  </Trans>
                )
              }
              action={
                !debouncedSearch && (
                  <Button onClick={handleCreate} size="lg">
                    <Plus className="mr-2 h-5 w-5" />
                    <Trans>Create Preset</Trans>
                  </Button>
                )
              }
            />
          )}
        </AnimatePresence>

        <ListPagination
          pageIndex={currentPage}
          totalCount={data?.total}
          pageSize={pageSize}
          onPageIndexChange={handlePageIndexChange}
          onPageSizeChange={(size) => updateSearch({ size, page: undefined })}
        />

        {/* Clone Dialog */}
        <Dialog open={cloneDialogOpen} onOpenChange={setCloneDialogOpen}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>
                <Trans>Clone Preset</Trans>
              </DialogTitle>
              <DialogDescription>
                <Trans>
                  Create a copy of "{presetToClone?.name}" with a new name.
                </Trans>
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-4">
              <div className="grid gap-2">
                <Label htmlFor="clone-name">
                  <Trans>New Preset Name</Trans>
                </Label>
                <Input
                  id="clone-name"
                  value={cloneName}
                  onChange={(e) => setCloneName(e.target.value)}
                  placeholder={i18n._(msg`Enter a unique name`)}
                />
              </div>
            </div>
            <DialogFooter>
              <Button
                variant="outline"
                onClick={() => setCloneDialogOpen(false)}
              >
                <Trans>Cancel</Trans>
              </Button>
              <Button
                onClick={handleCloneConfirm}
                disabled={!cloneName.trim() || cloneMutation.isPending}
              >
                {cloneMutation.isPending ? (
                  <Trans>Cloning...</Trans>
                ) : (
                  <Trans>Clone</Trans>
                )}
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>

        <CreateFab label={i18n._(msg`Create Preset`)} onClick={handleCreate} />
        <StepInUseDialog refusal={inUse} onClose={() => setInUse(undefined)} />
      </div>
    </div>
  );
}
