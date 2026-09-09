import { ReactNode, Suspense } from 'react';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { Clock, Combine, Layers } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { StatusInfoTooltip } from '@/components/shared/status-info-tooltip';
import { genericLazy } from '@/lib/generic-component';

// The step editor pulls in the flow-graph library, which is far larger than the rest of a config
// page, so it only loads once the pipeline tab is opened.
const PipelineConfigAdapter = genericLazy<
  typeof import('./pipeline-config-adapter').PipelineConfigAdapter
>(() =>
  import('./pipeline-config-adapter').then((m) => ({
    default: m.PipelineConfigAdapter,
  })),
);

interface PipelineTabsSectionProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /**
   * Field holding each pipeline. Omit `paired` or `session` for an entity that has no such
   * field; its tab then explains that the pipeline is unavailable.
   */
  names: {
    perSegment: Path<TFieldValues>;
    paired?: Path<TFieldValues>;
    session?: Path<TFieldValues>;
  };
  /**
   * Names stored inside each DAG definition. Supplied by the global settings page, whose
   * partial update also needs an empty DAG rather than `null` to clear a pipeline.
   */
  dagNames?: { perSegment?: string; paired?: string; session?: string };
  mode?: 'json' | 'object';
}

const TRIGGER_CLASS =
  'flex-1 min-w-[100px] sm:min-w-0 gap-2 rounded-lg text-muted-foreground hover:bg-muted aria-selected:!bg-primary aria-selected:!text-primary-foreground aria-selected:!shadow-md aria-selected:font-medium transition-all';

const TOOLTIP_CLASS =
  'p-0 border-border/50 shadow-xl bg-background/95 backdrop-blur-md overflow-hidden';

function EditorFallback() {
  return (
    <div className="flex items-center justify-center min-h-[500px] text-muted-foreground bg-background/20 backdrop-blur-sm border-white/5 rounded-lg border animate-pulse">
      <Trans>Loading editor components...</Trans>
    </div>
  );
}

/**
 * The three pipeline triggers over the step editor, shared by the global settings page and the
 * template / platform / streamer editors so both offer the same tabs and explanations.
 */
export function PipelineTabsSection<TFieldValues extends FieldValues>({
  form,
  names,
  dagNames,
  mode = 'object',
}: PipelineTabsSectionProps<TFieldValues>) {
  // Only the global settings page names its DAGs, and its update request treats a `null` field
  // as "leave unchanged", so emptying a pipeline there has to send an empty DAG. Override fields
  // clear to `null`, which is how they express "inherit".
  const emptyValue = dagNames ? 'dag' : 'null';

  return (
    <TooltipProvider>
      <Tabs defaultValue="per-segment" className="w-full">
        <TabsList className="flex flex-wrap sm:flex-nowrap sm:grid sm:grid-cols-3 mb-6 bg-muted/60 p-1 py-1 rounded-xl h-auto overflow-x-auto no-scrollbar">
          <Tooltip>
            <TooltipTrigger asChild>
              <TabsTrigger value="per-segment" className={TRIGGER_CLASS}>
                <Layers className="w-4 h-4" />
                <span className="hidden sm:inline">
                  <Trans>Per-segment</Trans>
                </span>
                <span className="sm:hidden text-xs">
                  <Trans>Segment</Trans>
                </span>
              </TabsTrigger>
            </TooltipTrigger>
            <TooltipContent className={TOOLTIP_CLASS}>
              <StatusInfoTooltip
                theme="blue"
                icon={<Layers className="w-4 h-4" />}
                title={<Trans>Per-segment Pipeline</Trans>}
                subtitle={<Trans>Triggered after each segment recording</Trans>}
              >
                <p className="text-xs text-muted-foreground leading-relaxed">
                  <Trans>
                    This pipeline runs immediately after a recording segment is
                    finished. Use it for tasks that only require the individual
                    video segment, such as remuxing, thumbnail generation, or
                    per-segment uploads.
                  </Trans>
                </p>
              </StatusInfoTooltip>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <TabsTrigger value="paired" className={TRIGGER_CLASS}>
                <Combine className="w-4 h-4" />
                <span className="hidden sm:inline">
                  <Trans>Paired Segment</Trans>
                </span>
                <span className="sm:hidden text-xs">
                  <Trans>Paired</Trans>
                </span>
              </TabsTrigger>
            </TooltipTrigger>
            <TooltipContent className={TOOLTIP_CLASS}>
              <StatusInfoTooltip
                theme="orange"
                icon={<Combine className="w-4 h-4" />}
                title={<Trans>Paired Segment Pipeline</Trans>}
                subtitle={
                  <Trans>Triggered when video and danmu are available</Trans>
                }
              >
                <div className="space-y-2">
                  <p className="text-xs text-muted-foreground leading-relaxed">
                    <Trans>
                      Runs when both the video segment and its corresponding
                      danmu segment are available.
                    </Trans>
                  </p>
                  <p className="text-xs font-medium text-orange-500/80">
                    <Trans>Requires "Record Danmu" to be enabled.</Trans>
                  </p>
                </div>
              </StatusInfoTooltip>
            </TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger asChild>
              <TabsTrigger value="session" className={TRIGGER_CLASS}>
                <Clock className="w-4 h-4" />
                <span className="hidden sm:inline">
                  <Trans>Session Complete</Trans>
                </span>
                <span className="sm:hidden text-xs">
                  <Trans>Session</Trans>
                </span>
              </TabsTrigger>
            </TooltipTrigger>
            <TooltipContent className={TOOLTIP_CLASS}>
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
                    Runs once after the recording session concludes and all
                    individual segment pipelines have finished. Ideal for
                    session-wide actions like merging all segments, final
                    notifications, or cleanup.
                  </Trans>
                </p>
              </StatusInfoTooltip>
            </TooltipContent>
          </Tooltip>
        </TabsList>

        <TabsContent value="per-segment" className="focus-visible:outline-none">
          <div className="space-y-4">
            <Alert className="bg-blue-500/5 border-blue-500/20 text-blue-600 dark:text-blue-400">
              <Layers className="h-4 w-4" />
              <AlertTitle>
                <Trans>Per-segment Pipeline</Trans>
              </AlertTitle>
              <AlertDescription className="text-xs">
                <Trans>
                  Runs for each recorded segment immediately after it's
                  finished.
                </Trans>
              </AlertDescription>
            </Alert>
            <PipelineEditor
              form={form}
              name={names.perSegment}
              dagName={dagNames?.perSegment}
              mode={mode}
              emptyValue={emptyValue}
            />
          </div>
        </TabsContent>

        <TabsContent value="paired" className="focus-visible:outline-none">
          <div className="space-y-4">
            <Alert className="bg-orange-500/5 border-orange-500/20 text-orange-600 dark:text-orange-400">
              <Combine className="h-4 w-4" />
              <AlertTitle>
                <Trans>Paired Segment Pipeline</Trans>
              </AlertTitle>
              <AlertDescription className="text-xs space-y-1">
                <p>
                  <Trans>
                    Runs when both video and danmu segments are available.
                  </Trans>
                </p>
                <p className="font-semibold">
                  <Trans>Requires "Record Danmu" to be enabled.</Trans>
                </p>
              </AlertDescription>
            </Alert>
            {names.paired ? (
              <PipelineEditor
                form={form}
                name={names.paired}
                dagName={dagNames?.paired}
                mode={mode}
                emptyValue={emptyValue}
              />
            ) : (
              <UnsupportedPipeline>
                <Trans>Paired pipeline is not supported for this entity.</Trans>
              </UnsupportedPipeline>
            )}
          </div>
        </TabsContent>

        <TabsContent value="session" className="focus-visible:outline-none">
          <div className="space-y-4">
            <Alert className="bg-indigo-500/5 border-indigo-500/20 text-indigo-600 dark:text-indigo-400">
              <Clock className="h-4 w-4" />
              <AlertTitle>
                <Trans>Session Complete Pipeline</Trans>
              </AlertTitle>
              <AlertDescription className="text-xs">
                <Trans>
                  Runs once after the entire session ends and all segment
                  pipelines have completed.
                </Trans>
              </AlertDescription>
            </Alert>
            {names.session ? (
              <PipelineEditor
                form={form}
                name={names.session}
                dagName={dagNames?.session}
                mode={mode}
                emptyValue={emptyValue}
              />
            ) : (
              <UnsupportedPipeline>
                <Trans>
                  Session complete pipeline is not supported for this entity.
                </Trans>
              </UnsupportedPipeline>
            )}
          </div>
        </TabsContent>
      </Tabs>
    </TooltipProvider>
  );
}

function PipelineEditor<TFieldValues extends FieldValues>({
  form,
  name,
  dagName,
  mode,
  emptyValue,
}: {
  form: UseFormReturn<TFieldValues>;
  name: Path<TFieldValues>;
  dagName?: string;
  mode: 'json' | 'object';
  emptyValue: 'null' | 'dag';
}) {
  return (
    <Suspense fallback={<EditorFallback />}>
      <PipelineConfigAdapter
        form={form}
        name={name}
        mode={mode}
        dagName={dagName}
        emptyValue={emptyValue}
      />
    </Suspense>
  );
}

/** Shown for an entity whose config has no field for that pipeline. */
function UnsupportedPipeline({ children }: { children: ReactNode }) {
  return (
    <div className="p-8 text-center text-muted-foreground border rounded-lg border-dashed">
      {children}
    </div>
  );
}
