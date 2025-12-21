import { UseFormReturn } from 'react-hook-form';
import { useEffect, useState } from 'react';
import { PipelineWorkflowEditor } from '@/components/pipeline/workflows/pipeline-workflow-editor';
import { DagStepDefinition, DagPipelineDefinition } from '@/api/schemas';

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
  useEffect(() => {
    // Debug log
    console.log('PipelineConfigAdapter: input changed', {
      currentVal,
      type: typeof currentVal,
      currentStepCount: steps.length,
    });

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
          // Sync local state with form state
          // JSON stringify comparison to avoid unnecessary re-renders/loops if objects are identical
          const currentJson = JSON.stringify(steps);
          const newJson = JSON.stringify(parsed.steps);

          if (currentJson !== newJson) {
            console.log('PipelineConfigAdapter: updating steps', {
              from: steps.length,
              to: parsed.steps.length,
            });
            setSteps(parsed.steps);
          }
        }
      } catch (e) {
        console.warn('Invalid Pipeline data', e);
      }
    } else {
      if (steps.length > 0) {
        console.log('PipelineConfigAdapter: clearing steps');
        setSteps([]);
      }
    }
  }, [currentVal, mode, steps]); // Added steps to dependency array for correct comparison

  const handleChange = (newSteps: DagStepDefinition[]) => {
    console.log('PipelineConfigAdapter: handleChange called', {
      count: newSteps.length,
    });
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

  return <PipelineWorkflowEditor steps={steps} onChange={handleChange} />;
}
