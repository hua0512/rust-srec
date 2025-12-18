import { motion } from 'motion/react';
import { UseFormReturn, FieldValues } from 'react-hook-form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../ui/tabs';
import { Card, CardContent } from '../ui/card';
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
} from 'lucide-react';
import { StreamSelectionTab } from './shared/stream-selection-tab';
import { LimitsCard } from './shared/limits-card';
import { OutputSettingsCard } from './shared/output-settings-card';
import { RecordDanmuCard } from './shared/record-danmu-card';
import { DanmuConfigForm } from './shared/danmu-config-form';

import { EventHooksForm } from './shared/event-hooks-form';
import { PipelineConfigAdapter } from './shared/pipeline-config-adapter';
import { NetworkSettingsCard } from './shared/network-settings-card';
import { ProxySettingsCard } from './shared/proxy-settings-card';

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
  // Danmu sampling config path (optional, as some entities don't support it)
  danmuSampling?: string;
  hooks: string;
  pipeline: string;
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
}: SharedConfigEditorProps<T>) {
  const showTab = (tab: ConfigTabType) => availableTabs.includes(tab);

  return (
    <Tabs defaultValue={defaultTab} className="w-full">
      <div className="bg-muted/50 p-0.5 sm:p-1 rounded-2xl backdrop-blur-sm border border-border/50 flex flex-col w-full">
        <TabsList className="h-auto bg-transparent p-0 gap-0.5 sm:gap-1 flex w-full overflow-x-auto no-scrollbar justify-start">
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
            >
              <Card className="border-dashed shadow-none">
                <CardContent className="p-4 sm:pt-6">
                  <StreamSelectionTab
                    form={form}
                    fieldName={paths.streamSelection}
                    mode={configMode}
                  />
                </CardContent>
              </Card>
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
            >
              <div className="space-y-6">
                <OutputSettingsCard
                  form={form}
                  basePath={paths.output === '' ? undefined : paths.output}
                  engines={engines}
                />
                <LimitsCard
                  form={form}
                  basePath={paths.limits === '' ? undefined : paths.limits}
                />
              </div>
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
            >
              <Card className="border-dashed shadow-none">
                <CardContent className="p-4 sm:pt-6">
                  <RecordDanmuCard
                    form={form}
                    basePath={paths.danmu === '' ? undefined : paths.danmu}
                  />

                  {paths.danmuSampling && (
                    <DanmuConfigForm
                      form={form}
                      name={paths.danmuSampling}
                      mode={configMode}
                    />
                  )}
                </CardContent>
              </Card>
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
            >
              <Card className="border-dashed shadow-none">
                <CardContent className="p-4 sm:pt-6">
                  <PipelineConfigAdapter
                    form={form}
                    name={paths.pipeline}
                    mode={configMode}
                  />
                </CardContent>
              </Card>
            </motion.div>
          </TabsContent>
        )}
      </div>
    </Tabs>
  );
}
