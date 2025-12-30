import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Textarea } from '@/components/ui/textarea';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Boxes, Code, List } from 'lucide-react';
import { useState, useEffect } from 'react';
import { Button } from '@/components/ui/button';
import {
  HuyaConfigSchema,
  DouyinConfigSchema,
  BilibiliConfigSchema,
  DouyuConfigSchema,
  TwitchConfigSchema,
  TikTokConfigSchema,
  TwitcastingConfigSchema,
} from '@/api/schemas';
import { HuyaConfigFields } from './specific-configs/HuyaConfigFields';
import { DouyinConfigFields } from './specific-configs/DouyinConfigFields';
import { BilibiliConfigFields } from './specific-configs/BilibiliConfigFields';
import { DouyuConfigFields } from './specific-configs/DouyuConfigFields';
import { TwitchConfigFields } from './specific-configs/TwitchConfigFields';
import { TikTokConfigFields } from './specific-configs/TikTokConfigFields';
import { TwitcastingConfigFields } from './specific-configs/TwitcastingConfigFields';

const PLATFORM_SCHEMAS: Record<string, any> = {
  huya: HuyaConfigSchema,
  douyin: DouyinConfigSchema,
  bilibili: BilibiliConfigSchema,
  douyu: DouyuConfigSchema,
  twitch: TwitchConfigSchema,
  tiktok: TikTokConfigSchema,
  twitcasting: TwitcastingConfigSchema,
};

const SPECIFIC_CONFIG_COMPONENTS: Record<string, any> = {
  huya: HuyaConfigFields,
  douyin: DouyinConfigFields,
  bilibili: BilibiliConfigFields,
  douyu: DouyuConfigFields,
  twitch: TwitchConfigFields,
  tiktok: TikTokConfigFields,
  twitcasting: TwitcastingConfigFields,
};

interface PlatformSpecificTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
  platformName?: string;
}

export function PlatformSpecificTab({
  form,
  basePath,
  platformName,
}: PlatformSpecificTabProps) {
  const fieldName = basePath
    ? `${basePath}.platform_specific_config`
    : 'platform_specific_config';

  const [viewMode, setViewMode] = useState<'form' | 'json'>('form');

  // Automatically switch to JSON view if no specific platform fields are available
  const hasSpecificFields =
    platformName && !!SPECIFIC_CONFIG_COMPONENTS[platformName.toLowerCase()];

  useEffect(() => {
    if (!hasSpecificFields) {
      setViewMode('json');
    }
  }, [hasSpecificFields]);

  const renderPlatformFields = () => {
    if (!platformName) return null;

    const name = platformName.toLowerCase();
    const SpecificFields = SPECIFIC_CONFIG_COMPONENTS[name];

    if (!SpecificFields) return null;

    return <SpecificFields form={form} fieldName={fieldName} />;
  };

  return (
    <Card className="border-dashed shadow-none">
      <CardHeader className="pb-4 border-b border-border/40">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 shrink-0">
              <Boxes className="w-5 h-5" />
            </div>
            <div className="grid gap-0.5 min-w-0">
              <CardTitle className="text-lg truncate">
                <Trans>Platform Specific Configuration</Trans>
              </CardTitle>
              <p className="text-xs text-muted-foreground truncate">
                {platformName ? (
                  <Trans>Options for {platformName}</Trans>
                ) : (
                  <Trans>General options for extractors</Trans>
                )}
              </p>
            </div>
          </div>
          {hasSpecificFields && (
            <div className="flex items-center p-1 bg-muted/50 rounded-lg border border-border/50 self-start sm:self-auto">
              <Button
                type="button"
                variant={viewMode === 'form' ? 'secondary' : 'ghost'}
                size="sm"
                className="h-8 gap-2 rounded-md transition-all flex-1 sm:flex-none"
                onClick={() => setViewMode('form')}
              >
                <List className="w-4 h-4" />
                <Trans>Form</Trans>
              </Button>
              <Button
                type="button"
                variant={viewMode === 'json' ? 'secondary' : 'ghost'}
                size="sm"
                className="h-8 gap-2 rounded-md transition-all flex-1 sm:flex-none"
                onClick={() => setViewMode('json')}
              >
                <Code className="w-4 h-4" />
                <Trans>JSON</Trans>
              </Button>
            </div>
          )}
        </div>
      </CardHeader>
      <CardContent className="pt-6">
        {viewMode === 'form' ? (
          <div className="animate-in fade-in duration-300">
            {renderPlatformFields() || (
              <div className="text-center py-12 text-muted-foreground border border-dashed rounded-lg">
                <Trans>No specific options available for this platform.</Trans>
              </div>
            )}
          </div>
        ) : (
          <FormField
            control={form.control}
            name={fieldName}
            render={({ field }) => {
              const [text, setText] = useState('');
              const [error, setError] = useState<string | null>(null);

              useEffect(() => {
                const val = field.value;
                if (val === null || val === undefined) {
                  setText('');
                } else if (typeof val === 'object') {
                  setText(JSON.stringify(val, null, 2));
                } else {
                  setText(String(val));
                }
              }, [field.value]);

              const handleChange = (
                e: React.ChangeEvent<HTMLTextAreaElement>,
              ) => {
                const newVal = e.target.value;
                setText(newVal);

                if (!newVal.trim()) {
                  field.onChange(null);
                  setError(null);
                  return;
                }

                try {
                  const parsed = JSON.parse(newVal);
                  const schema = platformName
                    ? PLATFORM_SCHEMAS[platformName.toLowerCase()]
                    : null;
                  if (schema) {
                    schema.parse(parsed);
                  }
                  field.onChange(parsed);
                  setError(null);
                  form.clearErrors(fieldName);
                } catch (err) {
                  setError((err as Error).message);
                  form.setError(fieldName, {
                    type: 'manual',
                    message: (err as Error).message,
                  });
                }
              };

              return (
                <div className="animate-in fade-in duration-300">
                  <FormItem>
                    <FormControl>
                      <div className="space-y-2">
                        <Textarea
                          value={text}
                          onChange={handleChange}
                          className="font-mono text-sm min-h-[400px] border-indigo-500/20 focus-visible:ring-indigo-500"
                          placeholder="{ ... }"
                        />
                        {error && (
                          <p className="text-sm font-medium text-destructive">
                            Invalid JSON: {error}
                          </p>
                        )}
                      </div>
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Raw JSON configuration specific to this platform.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                </div>
              );
            }}
          />
        )}
      </CardContent>
    </Card>
  );
}
