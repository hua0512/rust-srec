import { useMemo } from 'react';
import { useWatch } from 'react-hook-form';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  StreamSelectionInput,
  StreamSelectionConfig,
} from '../../streamers/config/stream-selection-input';
import { configPath, setConfigValue } from './form-path';
import { genericMemo } from '@/lib/generic-component';

interface StreamSelectionTabProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  basePath?: string;
  fieldName?: Path<TFieldValues>;
  mode?: 'json' | 'object';
}

function StreamSelectionTabImpl<TFieldValues extends FieldValues>({
  form,
  basePath,
  fieldName: propFieldName,
  mode = 'json',
}: StreamSelectionTabProps<TFieldValues>) {
  const fieldName =
    propFieldName ??
    configPath<TFieldValues>(basePath, 'stream_selection_config');

  const rawConfig = useWatch({ control: form.control, name: fieldName });

  const currentConfig: StreamSelectionConfig = useMemo(() => {
    if (!rawConfig) return {};

    // Handle string input (JSON) - useful if mode is json OR if data is unexpectedly a string in object mode
    if (typeof rawConfig === 'string') {
      try {
        return JSON.parse(rawConfig);
      } catch (e) {
        console.error('Failed to parse stream selection config:', e);
        return {};
      }
    }

    // Handle object input - useful if mode is object OR if data is unexpectedly an object in json mode
    if (typeof rawConfig === 'object') {
      return rawConfig;
    }

    return {};
  }, [rawConfig]);

  const handleConfigChange = (newConfig: StreamSelectionConfig) => {
    if (Object.keys(newConfig).length === 0) {
      if (mode === 'json') {
        setConfigValue(form, fieldName, null, {
          shouldDirty: true,
          shouldTouch: true,
        });
      } else {
        setConfigValue(form, fieldName, undefined, {
          shouldDirty: true,
          shouldTouch: true,
        });
      }
    } else {
      setConfigValue(
        form,
        fieldName,
        mode === 'json' ? JSON.stringify(newConfig) : newConfig,
        {
          shouldDirty: true,
          shouldTouch: true,
        },
      );
    }
  };

  return (
    <StreamSelectionInput value={currentConfig} onChange={handleConfigChange} />
  );
}

export const StreamSelectionTab = genericMemo(
  StreamSelectionTabImpl,
  'StreamSelectionTab',
);
