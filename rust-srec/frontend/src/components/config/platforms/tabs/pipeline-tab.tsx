import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { PipelineEditor } from '@/components/pipeline/editor/pipeline-editor';
import { Workflow, Plus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { StepLibrary } from '@/components/pipeline/workflows/step-library';

interface PipelineTabProps {
  form: UseFormReturn<any>;
}

export function PipelineTab({ form }: PipelineTabProps) {
  const rawPipeline = form.watch('pipeline');

  // Parse JSON string to array safely
  const getSteps = (raw: string | null | undefined): any[] => {
    if (!raw) return [];
    try {
      const parsed = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  };

  const currentSteps = getSteps(rawPipeline);

  const handleAddStep = (step: string) => {
    const newSteps = [...currentSteps, step];
    form.setValue('pipeline', JSON.stringify(newSteps), { shouldDirty: true });
  };

  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-pink-500/10 text-pink-600 dark:text-pink-400">
              <Workflow className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Pipeline Configuration</Trans>
              </CardTitle>
              <CardDescription>
                <Trans>Configure a custom processing pipeline.</Trans>
              </CardDescription>
            </div>
          </div>
          <StepLibrary
            onAddStep={handleAddStep}
            currentSteps={currentSteps.map((s) =>
              typeof s === 'string' ? s : s.processor,
            )}
            trigger={
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="gap-2"
              >
                <Plus className="h-4 w-4" />
                <Trans>Add Step</Trans>
              </Button>
            }
          />
        </div>
      </CardHeader>
      <CardContent>
        <FormField
          control={form.control}
          name="pipeline"
          render={({ field }) => {
            const steps = getSteps(field.value);

            const updateSteps = (newSteps: any[]) => {
              if (newSteps.length === 0) {
                field.onChange(null);
              } else {
                field.onChange(JSON.stringify(newSteps));
              }
            };

            return (
              <FormItem>
                <FormControl>
                  <PipelineEditor
                    steps={steps}
                    onChange={updateSteps}
                    hideAddButton={true}
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            );
          }}
        />
      </CardContent>
    </Card>
  );
}
