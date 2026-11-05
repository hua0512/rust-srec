import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { useCallback } from 'react';
import { queryKeys } from '@/api/query-keys';
import {
  listPipelinePresets,
  deletePipelinePreset,
} from '@/server/functions/pipeline';
import type { PipelinePreset } from '@/api/schemas';

import { Button } from '@/components/ui/button';
import { Plus, Workflow } from 'lucide-react';
import { toast } from 'sonner';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { WorkflowCard } from '@/components/pipeline/workflows/workflow-card';
import { SearchInput } from '@/components/shared/search-input';
import { useUpdateSearch } from '@/hooks/use-update-search';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { containerVariants, itemVariants } from '@/lib/animation';
import { ListPagination } from '@/components/shared/list-pagination';
import { PageEmptyState } from '@/components/shared/list-panel';
import { CreateFab } from '@/components/shared/page-actions';
import { QueryError } from '@/components/shared/query-error';
import { CARD_GRID } from '@/lib/grid';
import { cn } from '@/lib/utils';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/workflows/',
)({
  component: WorkflowsPage,
});

function WorkflowsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const updateSearch = useUpdateSearch<typeof search>();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  // Search + pagination live in the URL so they persist across navigation into
  // workflows/$workflowId and reloads.
  const debouncedSearch = search.q ?? '';
  const pageSize = search.size ?? 24;
  // The URL counts pages from 1; the pager counts them from 0.
  const currentPage = (search.page ?? 1) - 1;

  const { data, isLoading, isError, error, refetch } = useQuery({
    queryKey: queryKeys.pipeline.workflows.list({
      search: debouncedSearch,
      pageSize,
      page: currentPage,
    }),
    queryFn: () =>
      listPipelinePresets({
        data: {
          search: debouncedSearch || undefined,
          limit: pageSize,
          offset: currentPage * pageSize,
        },
      }),
    staleTime: 30000, // 30 seconds
    placeholderData: keepPreviousData,
  });

  const workflows = data?.presets;
  const totalCount = data?.total ?? 0;

  const handlePageIndexChange = useCallback(
    (index: number) =>
      updateSearch({ page: index > 0 ? index + 1 : undefined }),
    [updateSearch],
  );

  const deleteMutation = useMutation({
    mutationFn: deletePipelinePreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Workflow deleted successfully`));
      void queryClient.invalidateQueries({
        queryKey: queryKeys.pipeline.workflows.all,
      });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to delete workflow: ${error.message}`)),
  });

  const handleDelete = (id: string) => {
    deleteMutation.mutate({ data: id });
  };

  const handleEdit = (workflow: PipelinePreset) => {
    void navigate({
      to: '/pipeline/workflows/$workflowId',
      params: { workflowId: workflow.id },
    });
  };

  const handleCreate = () => {
    void navigate({ to: '/pipeline/workflows/create' });
  };

  return (
    <div className="min-h-screen space-y-6">
      {/* Header */}
      <DashboardHeader
        icon={Workflow}
        title={<Trans>Workflows</Trans>}
        subtitle={
          <Trans>
            Define sequences of processing steps for your recordings
          </Trans>
        }
        actions={
          <>
            <SearchInput
              defaultValue={debouncedSearch}
              onSearch={(value) =>
                updateSearch({ q: value || undefined, page: undefined })
              }
              placeholder={i18n._(msg`Search workflows...`)}
              className="flex-1 md:w-64 min-w-[200px]"
            />
            <Badge
              variant="secondary"
              className="h-9 px-3 text-sm whitespace-nowrap"
            >
              {totalCount} <Trans>workflows</Trans>
            </Badge>
          </>
        }
      />

      <div className="p-4 md:px-8 pb-20 w-full">
        <AnimatePresence mode="wait">
          {isError && !data ? (
            <QueryError
              key="error"
              title={<Trans>Failed to load workflows</Trans>}
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
              {[1, 2, 3].map((i) => (
                <div
                  key={i}
                  className="border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm overflow-hidden"
                >
                  <div className="flex justify-between items-start">
                    <Skeleton className="h-10 w-10 rounded-full" />
                    <Skeleton className="h-6 w-8" />
                  </div>
                  <div className="space-y-2 pt-2">
                    <Skeleton className="h-6 w-3/4" />
                    <Skeleton className="h-4 w-1/2" />
                  </div>
                  <div className="pt-4 mt-auto">
                    <Skeleton className="h-8 w-full rounded-md" />
                  </div>
                </div>
              ))}
            </motion.div>
          ) : workflows && workflows.length > 0 ? (
            <motion.div
              key="list"
              className={cn(CARD_GRID, 'gap-6')}
              variants={containerVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              {workflows.map((workflow) => (
                <motion.div key={workflow.id} variants={itemVariants}>
                  <WorkflowCard
                    workflow={workflow}
                    onEdit={handleEdit}
                    onDelete={handleDelete}
                  />
                </motion.div>
              ))}
            </motion.div>
          ) : (
            <PageEmptyState
              key="empty"
              icon={Workflow}
              title={
                debouncedSearch ? (
                  <Trans>No workflows found</Trans>
                ) : (
                  <Trans>No workflows yet</Trans>
                )
              }
              help={
                debouncedSearch ? (
                  <Trans>Try adjusting your search.</Trans>
                ) : (
                  <Trans>
                    Create your first workflow to define a sequence of
                    processing steps for your recordings.
                  </Trans>
                )
              }
              action={
                !debouncedSearch && (
                  <Button onClick={handleCreate} size="lg">
                    <Plus className="mr-2 h-5 w-5" />
                    <Trans>Create Workflow</Trans>
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

        <CreateFab
          label={i18n._(msg`Create Workflow`)}
          onClick={handleCreate}
        />
      </div>
    </div>
  );
}
