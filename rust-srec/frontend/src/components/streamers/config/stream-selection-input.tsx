import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { TagInput } from '@/components/ui/tag-input';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Filter, Zap } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

export interface StreamSelectionConfig {
  preferred_qualities?: string[];
  preferred_formats?: string[];
  preferred_cdns?: string[];
  blacklisted_cdns?: string[];
  min_bitrate?: number;
  max_bitrate?: number;
}

export interface StreamSelectionInputProps {
  value?: StreamSelectionConfig;
  onChange: (value: StreamSelectionConfig) => void;
}

export function StreamSelectionInput({
  value = {},
  onChange,
}: StreamSelectionInputProps) {
  const { i18n } = useLingui();
  const updateField = <K extends keyof StreamSelectionConfig>(
    field: K,
    fieldValue: StreamSelectionConfig[K],
  ) => {
    const newConfig = { ...value, [field]: fieldValue };

    // Cleanup empty arrays/undefined
    if (Array.isArray(fieldValue) && fieldValue.length === 0) {
      delete newConfig[field];
    }
    if (fieldValue === undefined || fieldValue === null) {
      // @ts-ignore
      delete newConfig[field];
    }

    onChange(newConfig);
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
                  value={value.preferred_qualities || []}
                  onChange={(tags) => updateField('preferred_qualities', tags)}
                  placeholder={i18n._(msg`e.g. 1080p, source, 原画`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>
                  Prioritize specific qualities (case-insensitive). Press Enter
                  to add.
                </Trans>
              </FormDescription>
            </FormItem>

            <FormItem>
              <FormLabel>
                <Trans>Preferred Formats</Trans>
              </FormLabel>
              <FormControl>
                <TagInput
                  value={value.preferred_formats || []}
                  onChange={(tags) => updateField('preferred_formats', tags)}
                  placeholder={i18n._(msg`e.g. flv, hls`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>
                  Prioritize specific streaming protocols. Press Enter to add.
                </Trans>
              </FormDescription>
            </FormItem>

            <FormItem>
              <FormLabel>
                <Trans>Preferred CDNs</Trans>
              </FormLabel>
              <FormControl>
                <TagInput
                  value={value.preferred_cdns || []}
                  onChange={(tags) => updateField('preferred_cdns', tags)}
                  placeholder={i18n._(msg`e.g. aliyun, akamaized`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>
                  Prioritize specific CDN providers. Press Enter to add.
                </Trans>
              </FormDescription>
            </FormItem>

            <FormItem>
              <FormLabel>
                <Trans>Blacklisted CDNs</Trans>
              </FormLabel>
              <FormControl>
                <TagInput
                  value={value.blacklisted_cdns || []}
                  onChange={(tags) => updateField('blacklisted_cdns', tags)}
                  placeholder={i18n._(msg`e.g. cdn-to-avoid`)}
                  className="bg-background"
                />
              </FormControl>
              <FormDescription>
                <Trans>
                  Exclude streams from these CDN providers entirely. Press Enter
                  to add.
                </Trans>
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
                min={0}
                value={value.min_bitrate ?? ''}
                onChange={(e) =>
                  updateField(
                    'min_bitrate',
                    e.target.value ? Number(e.target.value) : undefined,
                  )
                }
                placeholder={i18n._(msg`No limit`)}
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
                min={0}
                value={value.max_bitrate ?? ''}
                onChange={(e) =>
                  updateField(
                    'max_bitrate',
                    e.target.value ? Number(e.target.value) : undefined,
                  )
                }
                placeholder={i18n._(msg`No limit`)}
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
