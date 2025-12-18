import { UseFormReturn } from 'react-hook-form';
import { useEffect, useState } from 'react';
import { PipelineWorkflowEditor } from '@/components/pipeline/workflows/pipeline-workflow-editor';
import {
  DagStepDefinition,
  DagPipelineDefinition,
  PipelineStep,
} from '@/api/schemas';

interface PipelineConfigAdapterProps {
  form: UseFormReturn<any>;
  name: string;
  mode?: 'json' | 'object';
}

export function PipelineConfigAdapter({
  form,
  name,
  mode = 'object',
}: PipelineConfigAdapterProps) {
  const currentVal = form.watch(name);
  const [steps, setSteps] = useState<DagStepDefinition[]>([]);
  const [initialized, setInitialized] = useState(false);

  useEffect(() => {
    if (initialized) return;

    if (currentVal) {
      try {
        const parsed =
          mode === 'json' && typeof currentVal === 'string'
            ? JSON.parse(currentVal)
            : currentVal;

        if (
          parsed &&
          typeof parsed === 'object' &&
          Array.isArray(parsed.steps)
        ) {
          // New DAG format
          setSteps(parsed.steps);
        } else if (Array.isArray(parsed)) {
          // Legacy linear format - migrate to DAG
          const migratedSteps: DagStepDefinition[] = (
            parsed as PipelineStep[]
          ).map((step, i, arr) => {
            const stepName =
              step.type === 'inline' ? step.processor : step.name;
            return {
              id: `${stepName}-${i}`,
              step: step,
              depends_on:
                i > 0
                  ? [
                      `${(arr[i - 1] as PipelineStep).type === 'inline' ? (arr[i - 1] as any).processor : (arr[i - 1] as any).name}-${i - 1}`,
                    ]
                  : [],
            };
          });
          setSteps(migratedSteps);
        }
      } catch (e) {
        console.warn('Invalid Pipeline data', e);
        setSteps([]);
      }
    } else {
      setSteps([]);
    }
    setInitialized(true);
  }, [currentVal, mode, initialized]);

  const handleChange = (newSteps: DagStepDefinition[]) => {
    setSteps(newSteps);

    const dagConfig: DagPipelineDefinition = {
      name: 'pipeline',
      steps: newSteps,
    };

    const valueToSet = mode === 'json' ? JSON.stringify(dagConfig) : dagConfig;

    form.setValue(name, newSteps.length > 0 ? valueToSet : null, {
      shouldDirty: true,
      shouldTouch: true,
      shouldValidate: true,
    });
  };

  if (!initialized) return null;

  return <PipelineWorkflowEditor steps={steps} onChange={handleChange} />;
}
