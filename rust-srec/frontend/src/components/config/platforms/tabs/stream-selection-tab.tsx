import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { Filter, Zap } from 'lucide-react';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '../../../ui/form';
import { Input } from '../../../ui/input';
import { TagInput } from '../../../ui/tag-input';
import { StreamSelectionConfigObjectSchema } from '../../../../api/schemas';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

type StreamSelectionConfig = z.infer<typeof StreamSelectionConfigObjectSchema>;

interface StreamSelectionTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function StreamSelectionTab({
  form,
  basePath,
}: StreamSelectionTabProps) {
  const fieldName = basePath
    ? `${basePath}.stream_selection_config`
    : 'stream_selection_config';
  const rawConfig = form.watch(fieldName);

  // Helper to parse JSON string to object
  const parseConfig = (
    jsonString: string | null | undefined,
  ): StreamSelectionConfig => {
    if (!jsonString) return {};
    try {
      return JSON.parse(jsonString);
    } catch (e) {
      console.error('Failed to parse stream selection config:', e);
      return {};
    }
  };

  const currentConfig = parseConfig(rawConfig);

  // Generic handler to update a specific field in the JSON string
  const updateField = <K extends keyof StreamSelectionConfig>(
    field: K,
    value: StreamSelectionConfig[K],
  ) => {
    const newConfig = { ...currentConfig, [field]: value };
    // Remove empty arrays/undefined to keep JSON clean
    if (Array.isArray(value) && value.length === 0) {
      delete newConfig[field];
    }
    if (value === undefined || value === null) {
      delete newConfig[field];
    }

    // If object is empty, set to null
    if (Object.keys(newConfig).length === 0) {
      form.setValue(fieldName, null, { shouldDirty: true });
    } else {
      form.setValue(fieldName, JSON.stringify(newConfig), {
        shouldDirty: true,
      });
    }
  };

  return (
    <div className="grid gap-6">
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-purple-500/10 text-purple-600 dark:text-purple-400">
              <Filter className="w-5 h-5" />
            </div>
            <CardTitle className="text-lg">
              <Trans>Preferences</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="space-y-6">
          <div className="grid grid-cols-1 gap-6">
            <FormItem>
              <FormLabel>
                <Trans>Preferred Qualities</Trans>
              </FormLabel>
              <FormControl>
                <TagInput
                  value={currentConfig.preferred_qualities || []}
                  onChange={(tags) => updateField('preferred_qualities', tags)}
                  placeholder="e.g. 1080p, source, 原画"
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>Prioritize specific qualities (case-insensitive).</Trans>
              </FormDescription>
            </FormItem>

            <FormItem>
              <FormLabel>
                <Trans>Preferred Formats</Trans>
              </FormLabel>
              <FormControl>
                <TagInput
                  value={currentConfig.preferred_formats || []}
                  onChange={(tags) => updateField('preferred_formats', tags)}
                  placeholder="e.g. flv, hls"
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>Prioritize specific streaming protocols.</Trans>
              </FormDescription>
            </FormItem>

            <FormItem>
              <FormLabel>
                <Trans>Preferred CDNs</Trans>
              </FormLabel>
              <FormControl>
                <TagInput
                  value={currentConfig.preferred_cdns || []}
                  onChange={(tags) => updateField('preferred_cdns', tags)}
                  placeholder="e.g. aliyun, akamaized"
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>Prioritize specific CDN providers.</Trans>
              </FormDescription>
            </FormItem>
          </div>
        </CardContent>
      </Card>

      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-yellow-500/10 text-yellow-600 dark:text-yellow-400">
              <Zap className="w-5 h-5" />
            </div>
            <CardTitle className="text-lg">
              <Trans>Limits</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="grid grid-cols-1 sm:grid-cols-2 gap-6">
          <FormItem>
            <FormLabel>
              <Trans>Min Bitrate (bps)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                value={currentConfig.min_bitrate ?? ''}
                onChange={(e) =>
                  updateField(
                    'min_bitrate',
                    e.target.value ? Number(e.target.value) : undefined,
                  )
                }
                placeholder="No limit"
                className="bg-background"
              />
            </FormControl>
            <FormDescription>
              <Trans>Ignore streams below this bitrate.</Trans>
            </FormDescription>
          </FormItem>

          <FormItem>
            <FormLabel>
              <Trans>Max Bitrate (bps)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                value={currentConfig.max_bitrate ?? ''}
                onChange={(e) =>
                  updateField(
                    'max_bitrate',
                    e.target.value ? Number(e.target.value) : undefined,
                  )
                }
                placeholder="No limit"
                className="bg-background"
              />
            </FormControl>
            <FormDescription>
              <Trans>Ignore streams above this bitrate.</Trans>
            </FormDescription>
          </FormItem>
        </CardContent>
      </Card>
    </div>
  );
}
