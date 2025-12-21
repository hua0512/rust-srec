import { UseFormReturn } from 'react-hook-form';
import { useMemo } from 'react';
import {
  StreamSelectionInput,
  StreamSelectionConfig,
} from '../../streamers/config/stream-selection-input';

interface StreamSelectionTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
  fieldName?: string;
  mode?: 'json' | 'object';
}

export function StreamSelectionTab({
  form,
  basePath,
  fieldName: propFieldName,
  mode = 'json',
}: StreamSelectionTabProps) {
  const fieldName =
    propFieldName ??
    (basePath
      ? `${basePath}.stream_selection_config`
      : 'stream_selection_config');
  const rawConfig = form.watch(fieldName);

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
      // In object mode, we might want empty object {} or null/undefined?
      // Streamer schema expects optional object. {} is fine. undefined is fine.
      // Platform schema expects JSON string (nullable).
      if (mode === 'json') {
        form.setValue(fieldName, null, { shouldDirty: true });
      } else {
        // For object mode, if it's cleaner to remove the key, send undefined.
        // But if form default is {}, maybe {} is safer to avoid controlled input warnings if switching?
        // Let's send undefined to indicate "no selection".
        form.setValue(fieldName, undefined, { shouldDirty: true });
      }
    } else {
      form.setValue(
        fieldName,
        mode === 'json' ? JSON.stringify(newConfig) : newConfig,
        {
          shouldDirty: true,
        },
      );
    }
  };

  return (
    <StreamSelectionInput value={currentConfig} onChange={handleConfigChange} />
  );
}
