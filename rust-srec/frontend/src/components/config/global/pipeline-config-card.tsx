import { Control } from 'react-hook-form';
import { SettingsCard } from '../settings-card';
import { FormField, FormMessage } from '@/components/ui/form';
import { Trans } from '@lingui/react/macro';
import { DagStepDefinition, DagPipelineDefinition } from '@/api/schemas';
import { useState, useEffect, memo } from 'react';
import { useWatch } from 'react-hook-form';
import { Combine, Clock, Layers } from 'lucide-react';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { lazy, Suspense } from 'react';
const PipelineWorkflowEditor = lazy(() =>
  import('@/components/pipeline/workflows/pipeline-workflow-editor').then(
    (m) => ({ default: m.PipelineWorkflowEditor }),
  ),
);
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

interface PipelineConfigCardProps {
  control: Control<any>;
}

interface PipelineSectionProps {
  control: Control<any>;
  name: string;
  title: string;
  description: string;
  icon: any;
  alertColor: string;
  dagName: string;
}

const PipelineSection = memo(
  ({
    control,
    name,
    title,
    description,
    icon: Icon,
    alertColor,
    dagName,
  }: PipelineSectionProps) => {
    const fieldValue = useWatch({
      control,
      name,
    });

    const [currentSteps, setCurrentSteps] = useState<DagStepDefinition[]>([]);
    const [initialized, setInitialized] = useState(false);

    useEffect(() => {
      if (initialized) {
        if (fieldValue && typeof fieldValue === 'object') {
          const steps = (fieldValue?.steps || []) as DagStepDefinition[];
          // Only update if steps actually changed to prevent downward re-renders
          if (
            steps.length !== currentSteps.length ||
            JSON.stringify(steps) !== JSON.stringify(currentSteps)
          ) {
            setCurrentSteps(steps);
          }
        } else if (currentSteps.length > 0) {
          setCurrentSteps([]);
        }
        return;
      }

      let loadedSteps: DagStepDefinition[] = [];
      if (fieldValue) {
        if (typeof fieldValue === 'object' && Array.isArray(fieldValue.steps)) {
          loadedSteps = fieldValue.steps;
        } else if (typeof fieldValue === 'string') {
          try {
            const parsed = JSON.parse(fieldValue);
            if (
              parsed &&
              typeof parsed === 'object' &&
              Array.isArray(parsed.steps)
            ) {
              loadedSteps = parsed.steps;
            }
          } catch (e) {
            console.error('Failed to parse pipeline config string', e);
          }
        }
      }

      setCurrentSteps(loadedSteps);
      setInitialized(true);
    }, [fieldValue, initialized, currentSteps]);

    return (
      <div className="space-y-4">
        <Alert className={alertColor}>
          <Icon className="h-4 w-4" />
          <AlertTitle>{title}</AlertTitle>
          <AlertDescription className="text-xs">{description}</AlertDescription>
        </Alert>
        <FormField
          control={control}
          name={name}
          render={({ field }) => {
            const updateSteps = (newSteps: DagStepDefinition[]) => {
              setCurrentSteps(newSteps);
              const dagConfig: DagPipelineDefinition = {
                name: dagName,
                steps: newSteps,
              };
              field.onChange(dagConfig);
            };

            return initialized ? (
              <Suspense
                fallback={
                  <div className="flex items-center justify-center min-h-[500px] text-muted-foreground bg-background/20 backdrop-blur-sm border-white/5 rounded-lg border animate-pulse">
                    <Trans>Loading editor components...</Trans>
                  </div>
                }
              >
                <PipelineWorkflowEditor
                  steps={currentSteps}
                  onChange={updateSteps}
                />
              </Suspense>
            ) : (
              <div className="flex items-center justify-center min-h-[500px] text-muted-foreground bg-background/20 backdrop-blur-sm border-white/5 rounded-lg">
                <Trans>Loading pipeline editor...</Trans>
              </div>
            );
          }}
        />
      </div>
    );
  },
);

PipelineSection.displayName = 'PipelineSection';

export const PipelineConfigCard = memo(
  ({ control }: PipelineConfigCardProps) => {
    const { i18n } = useLingui();
    return (
      <div className="space-y-6">
        <SettingsCard
          title={<Trans>Pipeline Configuration</Trans>}
          description={
            <Trans>
              Default pipeline flow. Configure the sequence of processors for
              new jobs.
            </Trans>
          }
          icon={Layers}
          iconColor="text-orange-500"
          iconBgColor="bg-orange-500/10"
        >
          <div className="space-y-6">
            <TooltipProvider>
              <Tabs defaultValue="per-segment" className="w-full">
                <TabsList className="grid grid-cols-3 mb-6 bg-muted/60 p-1 rounded-xl">
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <TabsTrigger
                        value="per-segment"
                        className="gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all"
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
                          <Trans>Triggered after each segment recording</Trans>
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
                        className="gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all"
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
                        className="gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all"
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
                          <Trans>Triggered after the entire session ends</Trans>
                        }
                      >
                        <p className="text-xs text-muted-foreground leading-relaxed">
                          <Trans>
                            Runs once after the recording session concludes and
                            all individual segment pipelines have finished.
                            Ideal for session-wide actions like merging all
                            segments, final notifications, or cleanup.
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
                  <PipelineSection
                    control={control}
                    name="pipeline"
                    title={i18n._(msg`Per-segment Pipeline`)}
                    description={i18n._(
                      msg`Runs for each recorded segment immediately after it's finished.`,
                    )}
                    icon={Layers}
                    alertColor="bg-blue-500/5 border-blue-500/20 text-blue-600 dark:text-blue-400"
                    dagName="global_pipeline"
                  />
                </TabsContent>

                <TabsContent
                  value="paired"
                  className="focus-visible:outline-none"
                >
                  <PipelineSection
                    control={control}
                    name="paired_segment_pipeline"
                    title={i18n._(msg`Paired Segment Pipeline`)}
                    description={i18n._(
                      msg`Runs when both video and danmu segments are available. Requires "Record Danmu" to be enabled.`,
                    )}
                    icon={Combine}
                    alertColor="bg-orange-500/5 border-orange-500/20 text-orange-600 dark:text-orange-400"
                    dagName="global_paired_pipeline"
                  />
                </TabsContent>

                <TabsContent
                  value="session"
                  className="focus-visible:outline-none"
                >
                  <PipelineSection
                    control={control}
                    name="session_complete_pipeline"
                    title={i18n._(msg`Session Complete Pipeline`)}
                    description={i18n._(
                      msg`Runs once after the entire session ends and all segment pipelines have completed.`,
                    )}
                    icon={Clock}
                    alertColor="bg-indigo-500/5 border-indigo-500/20 text-indigo-600 dark:text-indigo-400"
                    dagName="global_session_pipeline"
                  />
                </TabsContent>
              </Tabs>
            </TooltipProvider>
            <FormMessage />
          </div>
        </SettingsCard>
      </div>
    );
  },
);

PipelineConfigCard.displayName = 'PipelineConfigCard';
