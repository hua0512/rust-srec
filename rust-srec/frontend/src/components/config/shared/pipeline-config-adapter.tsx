import { UseFormReturn } from 'react-hook-form';
import { useEffect, useState } from 'react';
import { PipelineEditor } from '@/components/pipeline/editor/pipeline-editor';

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
  const [steps, setSteps] = useState<any[]>([]);

  useEffect(() => {
    if (currentVal) {
      if (mode === 'json' && typeof currentVal === 'string') {
        try {
          const parsed = JSON.parse(currentVal);
          setSteps(Array.isArray(parsed) ? parsed : []);
        } catch (e) {
          console.warn('Invalid Pipeline JSON', e);
          setSteps([]);
        }
      } else if (Array.isArray(currentVal)) {
        setSteps(currentVal);
      }
    } else {
      setSteps([]);
    }
  }, [currentVal, mode]);

  const handleChange = (newSteps: any[]) => {
    setSteps(newSteps);

    if (mode === 'json') {
      form.setValue(
        name,
        newSteps.length > 0 ? JSON.stringify(newSteps) : null,
        {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        },
      );
    } else {
      form.setValue(name, newSteps, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
    }
  };

  return <PipelineEditor steps={steps} onChange={handleChange} />;
}
