import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { motion } from 'motion/react';
import {
  ArrowLeft,
  Workflow,
  Save,
  Settings2,
  Layout,
  List,
  Share2,
  CheckCircle2,
  ShieldCheck,
} from 'lucide-react';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';

import { Button } from '@/components/ui/button';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';

import { StepLibrary } from './step-library';
import { StepsList } from './steps-list';
import type { PipelinePreset } from '@/server/functions/pipeline';
import { validateDagDefinition } from '@/server/functions/pipeline';
import { toast } from 'sonner';
import {
  PipelineStep,
  DagStepDefinition,
  DagStepDefinitionSchema,
} from '@/api/schemas';
import { WorkflowFlowEditor } from './flow-editor/workflow-flow-editor';
import { StepConfigDialog } from './step-config-dialog';

const workflowSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  description: z.string().optional(),
  steps: z
    .array(DagStepDefinitionSchema)
    .min(1, 'At least one step is required'),
});

type WorkflowFormData = z.infer<typeof workflowSchema>;

interface WorkflowEditorProps {
  initialData?: PipelinePreset;
  title: React.ReactNode;
  onSubmit: (data: WorkflowFormData) => void;
  isUpdating?: boolean;
}

export function WorkflowEditor({
  initialData,
  title,
  onSubmit,
  isUpdating,
}: WorkflowEditorProps) {
  const navigate = useNavigate();
  const [viewMode, setViewMode] = useState<'list' | 'graph'>('list');
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [isValidating, setIsValidating] = useState(false);

  const initialSteps = initialData?.dag?.steps || [];

  const form = useForm<WorkflowFormData>({
    resolver: zodResolver(workflowSchema) as any,
    defaultValues: {
      name: initialData?.name || '',
      description: initialData?.description || '',
      steps: initialSteps,
    },
  });

  const steps = form.watch('steps');

  const handleAddStep = (pipelineStep: PipelineStep) => {
    const currentSteps = form.getValues('steps');
    const stepName =
      pipelineStep.type === 'inline'
        ? pipelineStep.processor
        : pipelineStep.name;
    const newStep: DagStepDefinition = {
      id: `${stepName}-${currentSteps.length}`,
      step: pipelineStep,
      depends_on:
        currentSteps.length > 0
          ? [currentSteps[currentSteps.length - 1].id]
          : [],
    };
    form.setValue('steps', [...currentSteps, newStep], {
      shouldDirty: true,
    });
  };

  const handleRemoveStep = (index: number) => {
    const currentSteps = form.getValues('steps');
    const removedStep = currentSteps[index];
    if (!removedStep) return;
    performRemoveStep(removedStep.id, currentSteps);
  };

  const handleRemoveStepById = (id: string) => {
    const currentSteps = form.getValues('steps');
    performRemoveStep(id, currentSteps);
  };

  const performRemoveStep = (id: string, currentSteps: DagStepDefinition[]) => {
    const removedStep = currentSteps.find((s) => s.id === id);
    if (!removedStep) return;

    // Implementation of bridging logic:
    // When a step is removed, its successors will now depend on its predecessors
    const predecessors = removedStep.depends_on || [];

    const updatedSteps = currentSteps
      .filter((s) => s.id !== id)
      .map((s) => {
        if (s.depends_on?.includes(id)) {
          // Remove the deleted step from dependencies and add its own predecessors
          const newDependsOn = [
            ...new Set([
              ...s.depends_on.filter((depId) => depId !== id),
              ...predecessors,
            ]),
          ];
          return { ...s, depends_on: newDependsOn };
        }
        return s;
      });

    form.setValue('steps', updatedSteps, { shouldDirty: true });
  };

  const handleReorder = (newOrder: DagStepDefinition[]) => {
    form.setValue('steps', newOrder, { shouldDirty: true });
  };

  const handleUpdateStep = (index: number, newStep: DagStepDefinition) => {
    const currentSteps = form.getValues('steps');
    const updatedSteps = [...currentSteps];
    updatedSteps[index] = newStep;
    form.setValue('steps', updatedSteps, { shouldDirty: true });
  };

  const handleEditStepById = (id: string) => {
    const index = steps.findIndex((s) => s.id === id);
    if (index !== -1) {
      setEditingIndex(index);
    }
  };

  const handleValidate = async () => {
    const data = form.getValues();
    if (data.steps.length === 0) {
      toast.error(t`Pipeline must have at least one step`);
      return;
    }

    setIsValidating(true);
    try {
      const result = await validateDagDefinition({
        data: {
          name: data.name,
          steps: data.steps,
        },
      });

      if (result.valid) {
        toast.success(t`Pipeline is valid`, {
          description: t`No errors found. Max depth: ${result.max_depth}`,
          icon: <CheckCircle2 className="h-4 w-4 text-green-500" />,
        });
      } else {
        toast.error(t`Validation Failed`, {
          description: (
            <div className="space-y-1 mt-1">
              {result.errors.map((e, i) => (
                <p
                  key={i}
                  className="text-xs font-mono bg-destructive/10 p-1 rounded text-destructive"
                >
                  {e}
                </p>
              ))}
              {result.warnings.map((e, i) => (
                <p
                  key={i}
                  className="text-xs font-mono bg-yellow-500/10 p-1 rounded text-yellow-500"
                >
                  {e}
                </p>
              ))}
            </div>
          ),
          duration: 5000,
        });
      }
    } catch (error) {
      console.error(error);
      toast.error(t`Validation service unavailable`);
    } finally {
      setIsValidating(false);
    }
  };

  return (
    <div className="min-h-screen flex flex-col">
      {/* Header */}
      <div className="border-b border-border/40">
        <div className="max-w-[1600px] mx-auto px-4 md:px-8 py-3 flex items-center justify-between">
          <div className="flex items-center gap-4">
            <Button
              variant="ghost"
              size="icon"
              onClick={() => navigate({ to: '/pipeline/workflows' })}
              className="shrink-0 rounded-full hover:bg-muted/60"
            >
              <ArrowLeft className="h-5 w-5" />
            </Button>
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
                <Workflow className="h-5 w-5 text-primary" />
              </div>
              <div>
                <h1 className="text-lg font-semibold tracking-tight">
                  {title}
                </h1>
                <p className="text-xs text-muted-foreground mr-6">
                  <Trans>Design your automation pipeline step by step</Trans>
                </p>
              </div>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={handleValidate}
              disabled={isValidating}
              className="hidden sm:flex"
            >
              {isValidating ? (
                <span className="animate-spin mr-2">⏳</span>
              ) : (
                <ShieldCheck className="h-4 w-4 mr-2" />
              )}
              <Trans>Validate</Trans>
            </Button>
            <Button
              onClick={form.handleSubmit(onSubmit)}
              disabled={isUpdating || !form.formState.isDirty}
              className="gap-2 shadow-lg shadow-primary/20"
            >
              <Save className="h-4 w-4" />
              {isUpdating ? (
                <Trans>Saving...</Trans>
              ) : (
                <Trans>Save Workflow</Trans>
              )}
            </Button>
          </div>
        </div>
      </div>

      <div className="flex-1 max-w-[1600px] mx-auto w-full px-4 md:px-8 py-8">
        <Form {...form}>
          <form
            onSubmit={form.handleSubmit(onSubmit)}
            className="grid grid-cols-1 lg:grid-cols-12 gap-8 h-full"
          >
            {/* Left Column: Basic Info & Toolbox */}
            <div className="lg:col-span-4 space-y-6">
              <motion.div
                initial={{ opacity: 0, x: -20 }}
                animate={{ opacity: 1, x: 0 }}
                className="group relative overflow-hidden rounded-2xl border border-border/40 bg-gradient-to-br from-background/50 to-background/20 backdrop-blur-xl transition-all hover:border-primary/20"
              >
                <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />
                <div className="p-6 space-y-6">
                  <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                    <Settings2 className="h-4 w-4 text-primary" />
                    <h3 className="font-medium tracking-tight">
                      <Trans>Configuration</Trans>
                    </h3>
                  </div>

                  <FormField
                    control={form.control}
                    name="name"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                          <Trans>Name</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            placeholder={t`e.g., Standard Processing`}
                            className="bg-muted/30 border-border/40 focus:bg-background/50 transition-colors"
                            {...field}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={form.control}
                    name="description"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                          <Trans>Description</Trans>
                        </FormLabel>
                        <FormControl>
                          <Textarea
                            placeholder={t`Describe what this workflow does...`}
                            className="resize-none bg-muted/30 border-border/40 focus:bg-background/50 transition-colors min-h-[120px]"
                            {...field}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </motion.div>

              <StepLibrary
                onAddStep={handleAddStep}
                currentSteps={steps.map((s) =>
                  s.step.type === 'inline' ? s.step.processor : s.step.name,
                )}
              />
            </div>

            {/* Right Column: Steps Visualizer */}
            <div className="lg:col-span-8 flex flex-col h-full min-h-[600px] space-y-4">
              <div className="flex items-center justify-between px-1 shrink-0">
                <div className="flex items-center gap-2">
                  <div className="p-2 rounded-lg bg-primary/10">
                    <Layout className="h-4 w-4 text-primary" />
                  </div>
                  <h3 className="font-semibold tracking-tight">
                    <Trans>Pipeline Structure</Trans>
                  </h3>
                </div>
                <div className="flex items-center gap-4">
                  <Tabs
                    value={viewMode}
                    onValueChange={(v) => setViewMode(v as any)}
                    className="h-9"
                  >
                    <TabsList className="grid w-full grid-cols-2 h-9 p-1">
                      <TabsTrigger value="list" className="h-7 px-4">
                        <List className="h-3.5 w-3.5 mr-2" />
                        <span className="text-xs">
                          <Trans>List</Trans>
                        </span>
                      </TabsTrigger>
                      <TabsTrigger value="graph" className="h-7 px-4">
                        <Share2 className="h-3.5 w-3.5 mr-2" />
                        <span className="text-xs">
                          <Trans>Graph</Trans>
                        </span>
                      </TabsTrigger>
                    </TabsList>
                  </Tabs>
                </div>
              </div>

              {viewMode === 'list' ? (
                <div className="flex-1">
                  <StepsList
                    steps={steps}
                    onReorder={handleReorder}
                    onRemove={handleRemoveStep}
                    onUpdate={handleUpdateStep}
                    onEdit={setEditingIndex}
                  />
                </div>
              ) : (
                <div className="flex-1 border border-border/40 rounded-2xl overflow-hidden bg-muted/5 relative min-h-[500px]">
                  <WorkflowFlowEditor
                    steps={steps}
                    onUpdateSteps={handleReorder}
                    onEditStep={handleEditStepById}
                    onRemoveStep={handleRemoveStepById}
                  />
                </div>
              )}
            </div>
          </form>
        </Form>
      </div>

      <StepConfigDialog
        open={editingIndex !== null}
        onOpenChange={(open) => !open && setEditingIndex(null)}
        dagStep={editingIndex !== null ? steps[editingIndex] : null}
        onSave={(data) => {
          if (editingIndex !== null) {
            handleUpdateStep(editingIndex, data);
            setEditingIndex(null);
          }
        }}
        allSteps={steps}
        currentStepIndex={editingIndex ?? -1}
      />
    </div>
  );
}
