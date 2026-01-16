import { useState, useEffect, memo } from 'react';
import { useInfiniteQuery } from '@tanstack/react-query';
import {
  Search,
  Tag,
  ArrowRight,
  LayoutGrid,
  CheckCircle2,
  Plus,
  Loader2,
  Workflow,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { motion, AnimatePresence } from 'motion/react';
import { useInView } from 'react-intersection-observer';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { ScrollArea } from '@/components/ui/scroll-area';
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/components/ui/sheet';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { getStepIcon } from '@/components/pipeline/constants';
import { listJobPresets } from '@/server/functions/job';
import { listPipelinePresets } from '@/server/functions/pipeline';
import { PipelineStep } from '@/api/schemas';
import { cn } from '@/lib/utils';
import {
  getCategoryName,
  getJobPresetDescription,
  getJobPresetName,
  getPipelinePresetDescription,
  getPipelinePresetName,
} from '@/components/pipeline/presets/default-presets-i18n';

interface StepLibraryProps {
  onAddStep: (step: PipelineStep) => void;
  currentSteps: string[];
  trigger?: React.ReactNode;
}

export const StepLibrary = memo(function StepLibrary({
  onAddStep,
  currentSteps,
  trigger,
}: StepLibraryProps) {
  const [searchQuery, setSearchQuery] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
  const [isOpen, setIsOpen] = useState(false);
  const [activeTab, setActiveTab] = useState('presets');

  const { i18n } = useLingui();

  const { ref: presetsRef, inView: presetsInView } = useInView();
  const { ref: workflowsRef, inView: workflowsInView } = useInView();

  // Debounce search
  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(searchQuery);
    }, 300);
    return () => clearTimeout(timer);
  }, [searchQuery]);

  // Fetch Job Presets
  const {
    data: presetsData,
    fetchNextPage: fetchNextPresets,
    hasNextPage: hasNextPresets,
    isFetchingNextPage: isFetchingNextPresets,
    isLoading: isLoadingPresets,
  } = useInfiniteQuery({
    queryKey: ['job', 'presets', debouncedSearch, selectedCategory],
    queryFn: ({ pageParam = 0 }) =>
      listJobPresets({
        data: {
          search: debouncedSearch || undefined,
          category: selectedCategory || undefined,
          limit: 20,
          offset: pageParam,
        },
      }),
    getNextPageParam: (lastPage) => {
      const nextOffset = lastPage.offset + lastPage.limit;
      return nextOffset < lastPage.total ? nextOffset : undefined;
    },
    initialPageParam: 0,
    enabled: activeTab === 'presets',
  });

  // Fetch Pipeline Presets (Workflows)
  const {
    data: workflowsData,
    fetchNextPage: fetchNextWorkflows,
    hasNextPage: hasNextWorkflows,
    isFetchingNextPage: isFetchingNextWorkflows,
    isLoading: isLoadingWorkflows,
  } = useInfiniteQuery({
    queryKey: ['pipeline', 'presets', debouncedSearch],
    queryFn: ({ pageParam = 0 }) =>
      listPipelinePresets({
        data: {
          search: debouncedSearch || undefined,
          limit: 20,
          offset: pageParam,
        },
      }),
    getNextPageParam: (lastPage) => {
      const nextOffset = lastPage.offset + lastPage.limit;
      return nextOffset < lastPage.total ? nextOffset : undefined;
    },
    initialPageParam: 0,
    enabled: activeTab === 'workflows',
  });

  // Auto-fetch when in view
  useEffect(() => {
    if (
      activeTab === 'presets' &&
      presetsInView &&
      hasNextPresets &&
      !isFetchingNextPresets
    ) {
      fetchNextPresets();
    }
  }, [
    activeTab,
    presetsInView,
    hasNextPresets,
    isFetchingNextPresets,
    fetchNextPresets,
  ]);

  useEffect(() => {
    if (
      activeTab === 'workflows' &&
      workflowsInView &&
      hasNextWorkflows &&
      !isFetchingNextWorkflows
    ) {
      fetchNextWorkflows();
    }
  }, [
    activeTab,
    workflowsInView,
    hasNextWorkflows,
    isFetchingNextWorkflows,
    fetchNextWorkflows,
  ]);

  // Flatten pages
  const presets = presetsData?.pages.flatMap((page) => page.presets) || [];
  const workflows = workflowsData?.pages.flatMap((page) => page.presets) || [];
  const categories = presetsData?.pages[0]?.categories || [];

  const handleAddPreset = (name: string) => {
    onAddStep({ type: 'preset', name });
  };

  const handleAddWorkflow = (name: string) => {
    onAddStep({ type: 'workflow', name });
  };

  const container = {
    hidden: { opacity: 0 },
    show: {
      opacity: 1,
      transition: {
        staggerChildren: 0.05,
      },
    },
  };

  const item = {
    hidden: { opacity: 0, y: 10 },
    show: {
      opacity: 1,
      y: 0,
      transition: { type: 'spring', stiffness: 300, damping: 24 } as const,
    },
    exit: {
      opacity: 0,
      scale: 0.93,
      transition: { duration: 0.2 },
    },
  };

  return (
    <Sheet open={isOpen} onOpenChange={setIsOpen}>
      <SheetTrigger asChild>
        {trigger ? (
          trigger
        ) : (
          <div className="group relative overflow-hidden rounded-2xl border border-border/40 bg-gradient-to-br from-background/50 to-background/20 backdrop-blur-xl transition-all hover:border-primary/20 cursor-pointer hover:shadow-lg hover:shadow-primary/5">
            <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />
            <div className="p-6 space-y-4">
              <div className="flex items-center justify-between pb-2 border-b border-border/40">
                <div className="flex items-center gap-2">
                  <div className="p-1.5 rounded-md bg-primary/10 ring-1 ring-primary/20">
                    <LayoutGrid className="h-4 w-4 text-primary" />
                  </div>
                  <h3 className="font-semibold tracking-tight text-foreground">
                    <Trans>Step Library</Trans>
                  </h3>
                </div>
                <div className="h-6 w-6 rounded-full bg-muted/50 flex items-center justify-center group-hover:bg-primary/10 transition-colors">
                  <ArrowRight className="h-3.5 w-3.5 text-muted-foreground group-hover:text-primary group-hover:translate-x-0.5 transition-all" />
                </div>
              </div>
              <div className="text-sm text-muted-foreground leading-relaxed">
                <Trans>
                  Explore matching automation steps to enhance your pipeline
                  workflow.
                </Trans>
              </div>
              <div className="flex items-center gap-2 text-xs font-medium text-muted-foreground/70 pt-1">
                <Tag className="h-3 w-3" />
                <span>
                  {presetsData?.pages[0]?.total ?? 0}{' '}
                  <Trans>Presets Available</Trans>
                </span>
              </div>
            </div>
          </div>
        )}
      </SheetTrigger>
      <SheetContent
        side="left"
        className="w-full sm:max-w-[600px] flex flex-col p-0 gap-0 border-r border-border/60 bg-background/95 backdrop-blur-2xl shadow-2xl"
      >
        <Tabs
          value={activeTab}
          onValueChange={setActiveTab}
          className="h-full flex flex-col"
        >
          <SheetHeader className="px-6 py-6 border-b border-border/40 space-y-4 bg-muted/5">
            <div className="space-y-1">
              <SheetTitle className="text-2xl font-bold tracking-tight">
                <Trans>Add Pipeline Step</Trans>
              </SheetTitle>
              <SheetDescription className="text-base">
                <Trans>
                  Select a processing job or workflow to add to your pipeline.
                </Trans>
              </SheetDescription>
            </div>

            <TabsList className="grid w-full grid-cols-2">
              <TabsTrigger value="presets">
                <Trans>Job Presets</Trans>
              </TabsTrigger>
              <TabsTrigger value="workflows">
                <Trans>Workflows</Trans>
              </TabsTrigger>
            </TabsList>

            <div className="relative">
              <Search className="absolute left-3.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground/60" />
              <Input
                placeholder={t`Search by name or description...`}
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="pl-10 h-11 bg-background border-border/50 focus:border-primary/50 focus:ring-primary/20 transition-all rounded-xl shadow-sm"
              />
            </div>

            {activeTab === 'presets' && categories.length > 0 && (
              <div className="-mx-6 px-6 overflow-hidden">
                <div className="flex gap-2 pb-1 overflow-x-auto no-scrollbar mask-gradient-r py-1">
                  <Button
                    variant={selectedCategory === null ? 'secondary' : 'ghost'}
                    size="sm"
                    onClick={() => setSelectedCategory(null)}
                    className={cn(
                      'h-8 rounded-full px-4 text-xs font-medium transition-all shadow-sm',
                      selectedCategory === null
                        ? 'bg-primary text-primary-foreground hover:bg-primary/90 shadow-md ring-2 ring-primary/20 ring-offset-1 ring-offset-background'
                        : 'bg-background border border-border/50 hover:bg-muted',
                    )}
                  >
                    <Trans>All</Trans>
                  </Button>
                  {categories.map((cat) => (
                    <Button
                      key={cat}
                      variant={selectedCategory === cat ? 'secondary' : 'ghost'}
                      size="sm"
                      onClick={() => setSelectedCategory(cat)}
                      className={cn(
                        'h-8 rounded-full px-4 text-xs font-medium transition-all shadow-sm',
                        selectedCategory === cat
                          ? 'bg-primary text-primary-foreground hover:bg-primary/90 shadow-md ring-2 ring-primary/20 ring-offset-1 ring-offset-background'
                          : 'bg-background border border-border/50 hover:bg-muted',
                      )}
                    >
                      {getCategoryName(cat, i18n)}
                    </Button>
                  ))}
                </div>
              </div>
            )}
          </SheetHeader>

          <TabsContent value="presets" className="flex-1 bg-muted/5 min-h-0">
            <ScrollArea className="h-full">
              <div className="p-6">
                {isLoadingPresets ? (
                  <div className="flex flex-col items-center justify-center py-20 space-y-4">
                    <Loader2 className="h-8 w-8 text-primary animate-spin" />
                    <p className="text-sm text-muted-foreground">
                      <Trans>Loading presets...</Trans>
                    </p>
                  </div>
                ) : (
                  <>
                    <motion.div
                      variants={container}
                      initial="hidden"
                      animate="show"
                      className="grid grid-cols-1 sm:grid-cols-2 gap-4"
                    >
                      <AnimatePresence mode="popLayout" initial={false}>
                        {presets.map((preset) => {
                          const Icon = getStepIcon(preset.processor);
                          const isAdded = currentSteps.includes(preset.name);

                          return (
                            <motion.button
                              layout
                              key={preset.id}
                              variants={item}
                              initial="hidden"
                              animate="show"
                              exit="exit"
                              onClick={() => handleAddPreset(preset.name)}
                              className={cn(
                                'group flex flex-col text-left gap-4 p-5 rounded-2xl border bg-card hover:bg-accent/5 transition-all relative overflow-hidden',
                                'border-border/50 hover:border-primary/30',
                                'shadow-sm hover:shadow-md hover:shadow-primary/5',
                                isAdded &&
                                  'ring-1 ring-green-500/30 bg-green-50/50 dark:bg-green-950/10',
                              )}
                            >
                              <div className="flex items-start justify-between w-full relative z-10">
                                <div
                                  className={cn(
                                    'p-2.5 rounded-xl border shadow-sm transition-transform group-hover:scale-110 duration-300',
                                    'bg-gradient-to-br from-background to-muted',
                                    isAdded
                                      ? 'border-green-200 dark:border-green-800 text-green-600 dark:text-green-400'
                                      : 'border-border/50 text-foreground/80',
                                  )}
                                >
                                  <Icon className="h-5 w-5" />
                                </div>
                                <div className="flex items-center gap-2">
                                  {preset.category && (
                                    <Badge
                                      variant="secondary"
                                      className="text-[10px] h-5 px-2 font-medium opacity-70 group-hover:opacity-100 transition-opacity bg-muted/50"
                                    >
                                      {preset.category}
                                    </Badge>
                                  )}
                                  {isAdded && (
                                    <motion.div
                                      initial={{ scale: 0.93, opacity: 0 }}
                                      animate={{ scale: 1, opacity: 1 }}
                                      className="h-5 w-5 rounded-full bg-green-500 flex items-center justify-center text-white shadow-sm"
                                    >
                                      <CheckCircle2 className="h-3 w-3" />
                                    </motion.div>
                                  )}
                                </div>
                              </div>

                              <div className="space-y-1.5 relative z-10">
                                <div className="font-semibold text-base tracking-tight text-foreground flex items-center justify-between">
                                  {getJobPresetName(preset, i18n)}
                                </div>
                                {getJobPresetDescription(preset, i18n) && (
                                  <p className="text-xs text-muted-foreground/80 line-clamp-2 leading-relaxed h-9">
                                    {getJobPresetDescription(preset, i18n)}
                                  </p>
                                )}
                                {!getJobPresetDescription(preset, i18n) && (
                                  <div className="h-9" />
                                )}
                              </div>

                              {/* Hover Action */}
                              <div className="absolute bottom-4 right-4 opacity-0 group-hover:opacity-100 transition-all transform group-hover:translate-x-0 translate-x-4">
                                <div className="h-8 w-8 rounded-full bg-primary flex items-center justify-center shadow-lg shadow-primary/20 text-primary-foreground">
                                  <Plus className="h-4 w-4" />
                                </div>
                              </div>

                              {/* Decorative Background */}
                              <div
                                className={cn(
                                  'absolute inset-0 opacity-0 group-hover:opacity-100 transition-opacity duration-500 pointer-events-none',
                                  'bg-gradient-to-br from-transparent via-transparent to-primary/5',
                                )}
                              />
                            </motion.button>
                          );
                        })}
                      </AnimatePresence>
                    </motion.div>

                    {presets.length === 0 && !isLoadingPresets && (
                      <div className="flex flex-col items-center justify-center py-20 text-center space-y-3 opacity-60">
                        <Search className="h-10 w-10 text-muted-foreground/40" />
                        <div className="space-y-1">
                          <p className="font-medium text-foreground">
                            <Trans>No presets found</Trans>
                          </p>
                          <p className="text-sm text-muted-foreground">
                            <Trans>Try adjusting your search or filters</Trans>
                          </p>
                        </div>
                      </div>
                    )}

                    {hasNextPresets && (
                      <div
                        ref={presetsRef}
                        className="flex justify-center py-6"
                      >
                        {isFetchingNextPresets && (
                          <div className="flex items-center gap-2 text-muted-foreground text-sm">
                            <Loader2 className="h-4 w-4 animate-spin" />
                            <Trans>Loading more...</Trans>
                          </div>
                        )}
                      </div>
                    )}
                  </>
                )}
              </div>
            </ScrollArea>
          </TabsContent>

          <TabsContent value="workflows" className="flex-1 bg-muted/5 min-h-0">
            <ScrollArea className="h-full">
              <div className="p-6">
                {isLoadingWorkflows ? (
                  <div className="flex flex-col items-center justify-center py-20 space-y-4">
                    <Loader2 className="h-8 w-8 text-primary animate-spin" />
                    <p className="text-sm text-muted-foreground">
                      <Trans>Loading workflows...</Trans>
                    </p>
                  </div>
                ) : (
                  <>
                    <motion.div
                      variants={container}
                      initial="hidden"
                      animate="show"
                      className="grid grid-cols-1 sm:grid-cols-2 gap-4"
                    >
                      <AnimatePresence mode="popLayout" initial={false}>
                        {workflows.map((workflow) => {
                          const isAdded = currentSteps.includes(workflow.name);

                          return (
                            <motion.button
                              layout
                              key={workflow.id}
                              variants={item}
                              initial="hidden"
                              animate="show"
                              exit="exit"
                              onClick={() => handleAddWorkflow(workflow.name)}
                              className={cn(
                                'group flex flex-col text-left gap-4 p-5 rounded-2xl border bg-card hover:bg-accent/5 transition-all relative overflow-hidden',
                                'border-border/50 hover:border-primary/30',
                                'shadow-sm hover:shadow-md hover:shadow-primary/5',
                                isAdded &&
                                  'ring-1 ring-green-500/30 bg-green-50/50 dark:bg-green-950/10',
                              )}
                            >
                              <div className="flex items-start justify-between w-full relative z-10">
                                <div
                                  className={cn(
                                    'p-2.5 rounded-xl border shadow-sm transition-transform group-hover:scale-110 duration-300',
                                    'bg-gradient-to-br from-background to-muted',
                                    isAdded
                                      ? 'border-green-200 dark:border-green-800 text-green-600 dark:text-green-400'
                                      : 'border-border/50 text-foreground/80',
                                  )}
                                >
                                  <Workflow className="h-5 w-5" />
                                </div>
                                <div className="flex items-center gap-2">
                                  {isAdded && (
                                    <motion.div
                                      initial={{ scale: 0.93, opacity: 0 }}
                                      animate={{ scale: 1, opacity: 1 }}
                                      className="h-5 w-5 rounded-full bg-green-500 flex items-center justify-center text-white shadow-sm"
                                    >
                                      <CheckCircle2 className="h-3 w-3" />
                                    </motion.div>
                                  )}
                                </div>
                              </div>

                              <div className="space-y-1.5 relative z-10">
                                <div className="font-semibold text-base tracking-tight text-foreground flex items-center justify-between">
                                  {getPipelinePresetName(workflow, i18n)}
                                </div>
                                {getPipelinePresetDescription(
                                  workflow,
                                  i18n,
                                ) && (
                                  <p className="text-xs text-muted-foreground/80 line-clamp-2 leading-relaxed h-9">
                                    {getPipelinePresetDescription(
                                      workflow,
                                      i18n,
                                    )}
                                  </p>
                                )}
                                {!getPipelinePresetDescription(
                                  workflow,
                                  i18n,
                                ) && <div className="h-9" />}
                              </div>

                              {/* Hover Action */}
                              <div className="absolute bottom-4 right-4 opacity-0 group-hover:opacity-100 transition-all transform group-hover:translate-x-0 translate-x-4">
                                <div className="h-8 w-8 rounded-full bg-primary flex items-center justify-center shadow-lg shadow-primary/20 text-primary-foreground">
                                  <Plus className="h-4 w-4" />
                                </div>
                              </div>

                              {/* Decorative Background */}
                              <div
                                className={cn(
                                  'absolute inset-0 opacity-0 group-hover:opacity-100 transition-opacity duration-500 pointer-events-none',
                                  'bg-gradient-to-br from-transparent via-transparent to-primary/5',
                                )}
                              />
                            </motion.button>
                          );
                        })}
                      </AnimatePresence>
                    </motion.div>

                    {workflows.length === 0 && !isLoadingWorkflows && (
                      <div className="flex flex-col items-center justify-center py-20 text-center space-y-3 opacity-60">
                        <Search className="h-10 w-10 text-muted-foreground/40" />
                        <div className="space-y-1">
                          <p className="font-medium text-foreground">
                            <Trans>No workflows found</Trans>
                          </p>
                          <p className="text-sm text-muted-foreground">
                            <Trans>Try adjusting your search or filters</Trans>
                          </p>
                        </div>
                      </div>
                    )}

                    {hasNextWorkflows && (
                      <div
                        ref={workflowsRef}
                        className="flex justify-center py-6"
                      >
                        {isFetchingNextWorkflows && (
                          <div className="flex items-center gap-2 text-muted-foreground text-sm">
                            <Loader2 className="h-4 w-4 animate-spin" />
                            <Trans>Loading more...</Trans>
                          </div>
                        )}
                      </div>
                    )}
                  </>
                )}
              </div>
            </ScrollArea>
          </TabsContent>
        </Tabs>
        <div className="p-4 border-t border-border/40 bg-muted/10 text-xs text-center text-muted-foreground/60">
          <Trans>Click to add. You can reorder steps in the editor.</Trans>
        </div>
      </SheetContent>
    </Sheet>
  );
});
