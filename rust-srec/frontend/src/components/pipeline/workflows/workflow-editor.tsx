import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { motion } from 'motion/react';
import { ArrowLeft, Workflow, Save, Settings2, Layout } from 'lucide-react';
import { useNavigate } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';

import { Button } from '@/components/ui/button';
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
import { PipelinePreset } from '@/server/functions/pipeline';
import { Badge } from '@/components/ui/badge';
import { PipelineStep, PipelineStepSchema } from '@/api/schemas';

const workflowSchema = z.object({
  name: z.string().min(1, 'Name is required'),
  description: z.string().optional(),
  steps: z.array(PipelineStepSchema).min(1, 'At least one step is required'),
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

  // Parse initial steps
  let initialSteps: PipelineStep[] = [];
  if (initialData?.steps) {
    try {
      initialSteps =
        typeof initialData.steps === 'string'
          ? JSON.parse(initialData.steps)
          : initialData.steps;
    } catch {}
  }

  const form = useForm<WorkflowFormData>({
    resolver: zodResolver(workflowSchema) as any,
    defaultValues: {
      name: initialData?.name || '',
      description: initialData?.description || '',
      steps: initialSteps,
    },
  });

  const steps = form.watch('steps');

  const handleAddStep = (presetName: string) => {
    const currentSteps = form.getValues('steps');
    form.setValue('steps', [...currentSteps, presetName], {
      shouldDirty: true,
    });
  };

  const handleRemoveStep = (index: number) => {
    const currentSteps = form.getValues('steps');
    form.setValue(
      'steps',
      currentSteps.filter((_, i) => i !== index),
      { shouldDirty: true },
    );
  };

  const handleReorder = (newOrder: PipelineStep[]) => {
    form.setValue('steps', newOrder, { shouldDirty: true });
  };

  const handleUpdateStep = (index: number, newStep: PipelineStep) => {
    const currentSteps = form.getValues('steps');
    const updatedSteps = [...currentSteps];
    updatedSteps[index] = newStep;
    form.setValue('steps', updatedSteps, { shouldDirty: true });
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
                  typeof s === 'string' ? s : s.processor,
                )}
              />
            </div>

            {/* Right Column: Steps Visualizer */}
            <div className="lg:col-span-8 flex flex-col h-full">
              <div className="flex items-center justify-between mb-4 px-1">
                <div className="flex items-center gap-2">
                  <div className="p-2 rounded-lg bg-primary/10">
                    <Layout className="h-4 w-4 text-primary" />
                  </div>
                  <h3 className="font-semibold tracking-tight">
                    <Trans>Pipeline Sequence</Trans>
                  </h3>
                </div>
                <Badge
                  variant="outline"
                  className="px-3 bg-background/50 backdrop-blur"
                >
                  {steps.length} <Trans>Steps</Trans>
                </Badge>
              </div>

              <div className="flex-1">
                <StepsList
                  steps={steps}
                  onReorder={handleReorder}
                  onRemove={handleRemoveStep}
                  onUpdate={handleUpdateStep}
                />
              </div>
            </div>
          </form>
        </Form>
      </div>
    </div>
  );
}
