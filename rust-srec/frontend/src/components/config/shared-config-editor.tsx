import { motion } from 'motion/react';
import { lazy, Suspense } from 'react';
import { UseFormReturn, FieldValues } from 'react-hook-form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../ui/tabs';
import { EngineConfig } from '@/api/schemas';
import { Trans } from '@lingui/react/macro';
import {
  Filter,
  FolderOutput,
  Network,
  MessageSquare,
  Shield,
  Webhook,
  Workflow,
  Combine,
  Clock,
  Layers,
} from 'lucide-react';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';
import { StreamSelectionTab } from './shared/stream-selection-tab';
import { LimitsCard } from './shared/limits-card';
import { OutputSettingsCard } from './shared/output-settings-card';
import { RecordDanmuCard } from './shared/record-danmu-card';

import { EventHooksForm } from './shared/event-hooks-form';
const PipelineConfigAdapter = lazy(() =>
  import('./shared/pipeline-config-adapter').then((m) => ({
    default: m.PipelineConfigAdapter,
  })),
);
import { NetworkSettingsCard } from './shared/network-settings-card';
import { ProxySettingsCard } from './shared/proxy-settings-card';
import { Alert, AlertDescription, AlertTitle } from '../ui/alert';
import type { CredentialSaveScope } from '@/server/functions/credentials';

export interface SharedConfigPaths {
  streamSelection: string;
  cookies: string;
  proxy: string;
  retryPolicy: string;
  // Output settings base path (folder, template, format, engine)
  output: string;
  // Limit settings base path (duration, sizes)
  limits: string;
  // Danmu settings base path (record_danmu)
  danmu: string;
  // Danmu sampling config path
  danmuSampling?: string;
  hooks: string;
  pipeline: string;
  sessionCompletePipeline?: string;
  pairedSegmentPipeline?: string;
}

export type ConfigTabType =
  | 'filters'
  | 'output'
  | 'network'
  | 'proxy'
  | 'hooks'
  | 'danmu'
  | 'pipeline';

export interface ExtraTab {
  value: string;
  label: React.ReactNode;
  icon?: React.ElementType;
  content: React.ReactNode;
}

interface SharedConfigEditorProps<T extends FieldValues> {
  form: UseFormReturn<T>;
  paths: SharedConfigPaths;
  engines?: EngineConfig[];
  availableTabs?: ConfigTabType[];
  defaultTab?: string;
  extraTabs?: ExtraTab[];
  // If true, assumes proxy_config is an object. If false/undefined, assumes JSON string logic usually,
  // but ProxyConfigSettings handles object output via props.
  proxyMode?: 'json' | 'object';
  // Mode for stream selection, retry policy, etc.
  configMode?: 'json' | 'object';
  streamerId?: string;
  credentialScope?: CredentialSaveScope;
  credentialPlatformNameHint?: string;
}

const tabContentVariants = {
  hidden: { opacity: 0, y: 10, scale: 0.98 },
  visible: {
    opacity: 1,
    y: 0,
    scale: 1,
    transition: {
      duration: 0.3,
      ease: 'easeOut' as const,
      staggerChildren: 0.1,
    },
  },
  exit: { opacity: 0, y: -10, scale: 0.98, transition: { duration: 0.2 } },
};

export function SharedConfigEditor<T extends FieldValues>({
  form,
  paths,
  engines,
  availableTabs = [
    'filters',
    'output',
    'network',
    'proxy',
    'hooks',
    'danmu',
    'pipeline',
  ],
  defaultTab = 'filters',
  extraTabs = [],
  proxyMode = 'object',
  configMode = 'object',
  streamerId,
  credentialScope,
  credentialPlatformNameHint,
}: SharedConfigEditorProps<T>) {
  const showTab = (tab: ConfigTabType) => availableTabs.includes(tab);

  return (
    <Tabs defaultValue={defaultTab} className="w-full">
      <div className="bg-muted/50 p-1 rounded-2xl backdrop-blur-sm border border-border/50 flex w-full relative group">
        <TabsList className="h-auto bg-transparent p-0 py-0.5 gap-1 flex w-full overflow-x-auto no-scrollbar justify-start scroll-smooth">
          {extraTabs.map((tab) => (
            <TabsTrigger
              key={tab.value}
              value={tab.value}
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              {tab.icon && <tab.icon className="w-3.5 h-3.5 sm:w-4 sm:h-4" />}
              <span className="text-xs sm:text-sm">{tab.label}</span>
            </TabsTrigger>
          ))}
          {showTab('filters') && (
            <TabsTrigger
              value="filters"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <Filter className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Filters</Trans>
              </span>
            </TabsTrigger>
          )}
          {showTab('output') && (
            <TabsTrigger
              value="output"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <FolderOutput className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Output</Trans>
              </span>
            </TabsTrigger>
          )}
          {showTab('network') && (
            <TabsTrigger
              value="network"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <Network className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Network</Trans>
              </span>
            </TabsTrigger>
          )}
          {showTab('proxy') && (
            <TabsTrigger
              value="proxy"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <Shield className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Proxy</Trans>
              </span>
            </TabsTrigger>
          )}
          {showTab('hooks') && (
            <TabsTrigger
              value="hooks"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <Webhook className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Hooks</Trans>
              </span>
            </TabsTrigger>
          )}
          {showTab('danmu') && (
            <TabsTrigger
              value="danmu"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <MessageSquare className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Danmu</Trans>
              </span>
            </TabsTrigger>
          )}
          {showTab('pipeline') && (
            <TabsTrigger
              value="pipeline"
              className="gap-1.5 sm:gap-2 px-3 sm:px-4 py-2 sm:py-2.5 h-9 sm:h-10 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm rounded-xl transition-all shrink-0"
            >
              <Workflow className="w-3.5 h-3.5 sm:w-4 sm:h-4" />
              <span className="text-xs sm:text-sm">
                <Trans>Pipeline</Trans>
              </span>
            </TabsTrigger>
          )}
        </TabsList>
      </div>

      <div className="mt-6">
        {extraTabs.map((tab) => (
          <TabsContent
            key={tab.value}
            value={tab.value}
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              {tab.content}
            </motion.div>
          </TabsContent>
        ))}

        {showTab('filters') && (
          <TabsContent
            value="filters"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="space-y-6"
            >
              <StreamSelectionTab
                form={form}
                fieldName={paths.streamSelection}
                mode={configMode}
              />
            </motion.div>
          </TabsContent>
        )}

        {showTab('output') && (
          <TabsContent
            value="output"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="space-y-6"
            >
              <OutputSettingsCard
                form={form}
                basePath={paths.output === '' ? undefined : paths.output}
                engines={engines}
              />
              <LimitsCard
                form={form}
                basePath={paths.limits === '' ? undefined : paths.limits}
              />
            </motion.div>
          </TabsContent>
        )}

        {showTab('network') && (
          <TabsContent
            value="network"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              <NetworkSettingsCard
                form={form}
                paths={{
                  cookies: paths.cookies,
                  retryPolicy: paths.retryPolicy,
                }}
                configMode={configMode}
                streamerId={streamerId}
                credentialScope={credentialScope}
                credentialPlatformNameHint={credentialPlatformNameHint}
              />
            </motion.div>
          </TabsContent>
        )}

        {showTab('proxy') && (
          <TabsContent
            value="proxy"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              <ProxySettingsCard
                form={form}
                name={paths.proxy}
                proxyMode={proxyMode}
              />
            </motion.div>
          </TabsContent>
        )}

        {showTab('hooks') && (
          <TabsContent
            value="hooks"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
            >
              <EventHooksForm
                form={form}
                name={paths.hooks}
                mode={configMode}
              />
            </motion.div>
          </TabsContent>
        )}

        {showTab('danmu') && (
          <TabsContent
            value="danmu"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="space-y-6"
            >
              <RecordDanmuCard
                form={form}
                basePath={paths.danmu === '' ? undefined : paths.danmu}
              />
            </motion.div>
          </TabsContent>
        )}

        {showTab('pipeline') && (
          <TabsContent
            value="pipeline"
            className="mt-0 focus-visible:outline-none"
          >
            <motion.div
              variants={tabContentVariants}
              initial="hidden"
              animate="visible"
              exit="exit"
              className="space-y-6"
            >
              <TooltipProvider>
                <Tabs defaultValue="per-segment" className="w-full">
                  <TabsList className="flex flex-wrap sm:flex-nowrap sm:grid sm:grid-cols-3 mb-6 bg-muted/60 p-1 py-1 rounded-xl h-auto overflow-x-auto no-scrollbar">
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <TabsTrigger
                          value="per-segment"
                          className="flex-1 min-w-[100px] sm:min-w-0 gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all"
                        >
                          <Layers className="w-4 h-4" />
                          <span className="hidden sm:inline">
                            <Trans>Per-segment</Trans>
                          </span>
                          <span className="sm:hidden text-xs">
                            <Trans>Segment</Trans>
                          </span>
                        </TabsTrigger>
                      </TooltipTrigger>
                      <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                        <StatusInfoTooltip
                          theme="blue"
                          icon={<Layers className="w-4 h-4" />}
                          title={<Trans>Per-segment Pipeline</Trans>}
                          subtitle={
                            <Trans>
                              Triggered after each segment recording
                            </Trans>
                          }
                        >
                          <p className="text-xs text-muted-foreground leading-relaxed">
                            <Trans>
                              This pipeline runs immediately after a recording
                              segment is finished. Use it for tasks that only
                              require the individual video segment, such as
                              remuxing, thumbnail generation, or per-segment
                              uploads.
                            </Trans>
                          </p>
                        </StatusInfoTooltip>
                      </TooltipContent>
                    </Tooltip>

                    <Tooltip>
                      <TooltipTrigger asChild>
                        <TabsTrigger
                          value="paired"
                          className="flex-1 min-w-[100px] sm:min-w-0 gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all"
                        >
                          <Combine className="w-4 h-4" />
                          <span className="hidden sm:inline">
                            <Trans>Paired Segment</Trans>
                          </span>
                          <span className="sm:hidden text-xs">
                            <Trans>Paired</Trans>
                          </span>
                        </TabsTrigger>
                      </TooltipTrigger>
                      <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                        <StatusInfoTooltip
                          theme="orange"
                          icon={<Combine className="w-4 h-4" />}
                          title={<Trans>Paired Segment Pipeline</Trans>}
                          subtitle={
                            <Trans>
                              Triggered when video and danmu are available
                            </Trans>
                          }
                        >
                          <div className="space-y-2">
                            <p className="text-xs text-muted-foreground leading-relaxed">
                              <Trans>
                                Runs when both the video segment and its
                                corresponding danmu segment are available.
                              </Trans>
                            </p>
                            <p className="text-xs font-medium text-orange-500/80">
                              <Trans>
                                Requires "Record Danmu" to be enabled.
                              </Trans>
                            </p>
                          </div>
                        </StatusInfoTooltip>
                      </TooltipContent>
                    </Tooltip>

                    <Tooltip>
                      <TooltipTrigger asChild>
                        <TabsTrigger
                          value="session"
                          className="flex-1 min-w-[100px] sm:min-w-0 gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all"
                        >
                          <Clock className="w-4 h-4" />
                          <span className="hidden sm:inline">
                            <Trans>Session Complete</Trans>
                          </span>
                          <span className="sm:hidden text-xs">
                            <Trans>Session</Trans>
                          </span>
                        </TabsTrigger>
                      </TooltipTrigger>
                      <TooltipContent className="p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden">
                        <StatusInfoTooltip
                          theme="violet"
                          icon={<Clock className="w-4 h-4" />}
                          title={<Trans>Session Complete Pipeline</Trans>}
                          subtitle={
                            <Trans>
                              Triggered after the entire session ends
                            </Trans>
                          }
                        >
                          <p className="text-xs text-muted-foreground leading-relaxed">
                            <Trans>
                              Runs once after the recording session concludes
                              and all individual segment pipelines have
                              finished. Ideal for session-wide actions like
                              merging all segments, final notifications, or
                              cleanup.
                            </Trans>
                          </p>
                        </StatusInfoTooltip>
                      </TooltipContent>
                    </Tooltip>
                  </TabsList>

                  <TabsContent
                    value="per-segment"
                    className="focus-visible:outline-none"
                  >
                    <div className="space-y-4">
                      <Alert className="bg-blue-500/5 border-blue-500/20 text-blue-600 dark:text-blue-400">
                        <Layers className="h-4 w-4" />
                        <AlertTitle>
                          <Trans>Per-segment Pipeline</Trans>
                        </AlertTitle>
                        <AlertDescription className="text-xs">
                          <Trans>
                            Runs for each recorded segment immediately after
                            it's finished.
                          </Trans>
                        </AlertDescription>
                      </Alert>
                      <Suspense
                        fallback={
                          <div className="h-[400px] w-full bg-muted/20 animate-pulse rounded-lg" />
                        }
                      >
                        <PipelineConfigAdapter
                          form={form}
                          name={paths.pipeline}
                          mode={configMode}
                        />
                      </Suspense>
                    </div>
                  </TabsContent>

                  <TabsContent
                    value="paired"
                    className="focus-visible:outline-none"
                  >
                    <div className="space-y-4">
                      <Alert className="bg-orange-500/5 border-orange-500/20 text-orange-600 dark:text-orange-400">
                        <Combine className="h-4 w-4" />
                        <AlertTitle>
                          <Trans>Paired Segment Pipeline</Trans>
                        </AlertTitle>
                        <AlertDescription className="text-xs space-y-1">
                          <p>
                            <Trans>
                              Runs when both video and danmu segments are
                              available.
                            </Trans>
                          </p>
                          <p className="font-semibold">
                            <Trans>
                              Requires "Record Danmu" to be enabled.
                            </Trans>
                          </p>
                        </AlertDescription>
                      </Alert>
                      {paths.pairedSegmentPipeline ? (
                        <Suspense
                          fallback={
                            <div className="h-[400px] w-full bg-muted/20 animate-pulse rounded-lg" />
                          }
                        >
                          <PipelineConfigAdapter
                            form={form}
                            name={paths.pairedSegmentPipeline}
                            mode={configMode}
                          />
                        </Suspense>
                      ) : (
                        <div className="p-8 text-center text-muted-foreground border rounded-lg border-dashed">
                          <Trans>
                            Paired pipeline is not supported for this entity.
                          </Trans>
                        </div>
                      )}
                    </div>
                  </TabsContent>

                  <TabsContent
                    value="session"
                    className="focus-visible:outline-none"
                  >
                    <div className="space-y-4">
                      <Alert className="bg-indigo-500/5 border-indigo-500/20 text-indigo-600 dark:text-indigo-400">
                        <Clock className="h-4 w-4" />
                        <AlertTitle>
                          <Trans>Session Complete Pipeline</Trans>
                        </AlertTitle>
                        <AlertDescription className="text-xs">
                          <Trans>
                            Runs once after the entire session ends and all
                            segment pipelines have completed.
                          </Trans>
                        </AlertDescription>
                      </Alert>
                      {paths.sessionCompletePipeline ? (
                        <Suspense
                          fallback={
                            <div className="h-[400px] w-full bg-muted/20 animate-pulse rounded-lg" />
                          }
                        >
                          <PipelineConfigAdapter
                            form={form}
                            name={paths.sessionCompletePipeline}
                            mode={configMode}
                          />
                        </Suspense>
                      ) : (
                        <div className="p-8 text-center text-muted-foreground border rounded-lg border-dashed">
                          <Trans>
                            Session complete pipeline is not supported for this
                            entity.
                          </Trans>
                        </div>
                      )}
                    </div>
                  </TabsContent>
                </Tabs>
              </TooltipProvider>
            </motion.div>
          </TabsContent>
        )}
      </div>
    </Tabs>
  );
}
