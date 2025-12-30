import { memo, useMemo } from 'react';
import { UseFormReturn, useWatch } from 'react-hook-form';
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

export const StreamSelectionTab = memo(
  ({
    form,
    basePath,
    fieldName: propFieldName,
    mode = 'json',
  }: StreamSelectionTabProps) => {
    const fieldName =
      propFieldName ??
      (basePath
        ? `${basePath}.stream_selection_config`
        : 'stream_selection_config');

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
          form.setValue(fieldName, null, {
            shouldDirty: true,
            shouldTouch: true,
          });
        } else {
          form.setValue(fieldName, undefined, {
            shouldDirty: true,
            shouldTouch: true,
          });
        }
      } else {
        form.setValue(
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
      <StreamSelectionInput
        value={currentConfig}
        onChange={handleConfigChange}
      />
    );
  },
);

StreamSelectionTab.displayName = 'StreamSelectionTab';
