import { createLazyFileRoute } from '@tanstack/react-router';
import {
  useQuery,
  useMutation,
  useQueryClient,
  keepPreviousData,
} from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { useMemo } from 'react';
import {
  listPipelinePresets,
  deletePipelinePreset,
  type PipelinePreset,
} from '@/server/functions/pipeline';

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
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { AlertCircle } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { DashboardHeader } from '@/components/shared/dashboard-header';
import { containerVariants, itemVariants } from '@/lib/animation';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination';

export const Route = createLazyFileRoute(
  '/_authed/_dashboard/pipeline/workflows/',
)({
  component: WorkflowsPage,
});

const PAGE_SIZES = [12, 24, 48, 96];

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
  const currentPage = search.page ?? 0;

  const { data, isLoading, isError, error } = useQuery({
    queryKey: ['pipeline', 'workflows', debouncedSearch, pageSize, currentPage],
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
  const totalPages = Math.ceil(totalCount / pageSize);

  // Memoize pagination pages calculation
  const paginationPages = useMemo(() => {
    const pages: (number | 'ellipsis')[] = [];
    if (totalPages <= 7) {
      for (let i = 0; i < totalPages; i++) pages.push(i);
    } else {
      pages.push(0);
      if (currentPage > 2) pages.push('ellipsis');
      for (
        let i = Math.max(1, currentPage - 1);
        i <= Math.min(totalPages - 2, currentPage + 1);
        i++
      ) {
        pages.push(i);
      }
      if (currentPage < totalPages - 3) pages.push('ellipsis');
      pages.push(totalPages - 1);
    }
    return pages;
  }, [totalPages, currentPage]);

  const deleteMutation = useMutation({
    mutationFn: deletePipelinePreset,
    onSuccess: () => {
      toast.success(i18n._(msg`Workflow deleted successfully`));
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'workflows'],
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

  if (isError) {
    return (
      <div className="space-y-8 p-6 md:p-10 max-w-7xl mx-auto">
        <Alert variant="destructive">
          <AlertCircle className="h-4 w-4" />
          <AlertTitle>
            <Trans>Error</Trans>
          </AlertTitle>
          <AlertDescription>
            <Trans>Failed to load workflows: {error.message}</Trans>
          </AlertDescription>
        </Alert>
      </div>
    );
  }

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
              className="flex-1 md:w-64"
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
          {isLoading ? (
            <motion.div
              key="loading"
              initial={false}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.1 } }}
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6"
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
              className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6"
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
            <motion.div
              key="empty"
              initial={{ opacity: 0, scale: 0.95 }}
              animate={{ opacity: 1, scale: 1 }}
              className="flex flex-col items-center justify-center py-32 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5 backdrop-blur-sm shadow-sm"
            >
              <div className="p-6 bg-primary/5 rounded-full ring-1 ring-primary/10">
                <Workflow className="h-16 w-16 text-primary/60" />
              </div>
              <div className="space-y-2 max-w-md">
                <h3 className="font-semibold text-2xl tracking-tight">
                  {debouncedSearch ? (
                    <Trans>No workflows found</Trans>
                  ) : (
                    <Trans>No workflows yet</Trans>
                  )}
                </h3>
                <p className="text-muted-foreground">
                  {debouncedSearch ? (
                    <Trans>Try adjusting your search.</Trans>
                  ) : (
                    <Trans>
                      Create your first workflow to define a sequence of
                      processing steps for your recordings.
                    </Trans>
                  )}
                </p>
              </div>
              {!debouncedSearch && (
                <Button onClick={handleCreate} size="lg" className="mt-4">
                  <Plus className="mr-2 h-5 w-5" />
                  <Trans>Create Workflow</Trans>
                </Button>
              )}
            </motion.div>
          )}
        </AnimatePresence>

        {/* Pagination Controls */}
        {totalPages > 1 && (
          <div className="flex items-center justify-between mt-8 pt-6 border-t">
            <div className="flex items-center gap-2">
              <span className="text-sm text-muted-foreground">
                <Trans>Per page:</Trans>
              </span>
              <Select
                value={pageSize.toString()}
                onValueChange={(value) => {
                  updateSearch({ size: Number(value), page: undefined });
                }}
              >
                <SelectTrigger className="w-20 h-8">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PAGE_SIZES.map((size) => (
                    <SelectItem key={size} value={size.toString()}>
                      {size}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <Pagination>
              <PaginationContent>
                <PaginationItem>
                  <PaginationPrevious
                    onClick={() =>
                      updateSearch({ page: Math.max(0, currentPage - 1) })
                    }
                    className={
                      currentPage === 0
                        ? 'pointer-events-none opacity-50'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>

                {paginationPages.map((page, idx) =>
                  page === 'ellipsis' ? (
                    <PaginationItem key={`ellipsis-${idx}`}>
                      <PaginationEllipsis />
                    </PaginationItem>
                  ) : (
                    <PaginationItem key={page}>
                      <PaginationLink
                        isActive={currentPage === page}
                        onClick={() => updateSearch({ page })}
                        className="cursor-pointer"
                      >
                        {page + 1}
                      </PaginationLink>
                    </PaginationItem>
                  ),
                )}

                <PaginationItem>
                  <PaginationNext
                    onClick={() =>
                      updateSearch({
                        page: Math.min(totalPages - 1, currentPage + 1),
                      })
                    }
                    className={
                      currentPage >= totalPages - 1
                        ? 'pointer-events-none opacity-50'
                        : 'cursor-pointer'
                    }
                  />
                </PaginationItem>
              </PaginationContent>
            </Pagination>
          </div>
        )}

        {/* Floating Action Button */}
        <motion.div
          className="fixed bottom-8 right-8 z-50"
          initial={{ scale: 0.93, rotate: 90, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, opacity: 1 }}
          whileHover={{ scale: 1.1 }}
          whileTap={{ scale: 0.9 }}
        >
          <Button
            onClick={handleCreate}
            size="icon"
            className="h-14 w-14 rounded-full shadow-2xl bg-primary hover:bg-primary/90 text-primary-foreground flex items-center justify-center p-0"
          >
            <Plus className="h-6 w-6" />
          </Button>
        </motion.div>
      </div>
    </div>
  );
}
