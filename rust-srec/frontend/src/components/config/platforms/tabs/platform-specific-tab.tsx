import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Textarea } from '@/components/ui/textarea';
import { Card, CardContent } from '@/components/ui/card';
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
import { HuyaConfigFields } from './specific-configs/huya-config-fields';
import { DouyinConfigFields } from './specific-configs/douyin-config-fields';
import { BilibiliConfigFields } from './specific-configs/bilibili-config-fields';
import { DouyuConfigFields } from './specific-configs/douyu-config-fields';
import { TwitchConfigFields } from './specific-configs/twitch-config-fields';
import { TikTokConfigFields } from './specific-configs/tiktok-config-fields';
import { TwitcastingConfigFields } from './specific-configs/twitcasting-config-fields';

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
    <Card className="border-border/50 shadow-md overflow-hidden transition-all duration-300">
      {/* Premium Header Banner */}
      <div className="border-b border-border/40 px-6 py-5 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
        <div className="flex items-center gap-4">
          <div className="p-2.5 rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 shrink-0 border border-indigo-500/10">
            <Boxes className="w-5 h-5" />
          </div>
          <div className="grid gap-0.5 min-w-0">
            <h3 className="text-lg font-bold tracking-tight text-foreground truncate">
              {platformName ? (
                <Trans>
                  {platformName.charAt(0).toUpperCase() + platformName.slice(1)}{' '}
                  Configuration
                </Trans>
              ) : (
                <Trans>Platform Specific Configuration</Trans>
              )}
            </h3>
            <p className="text-xs text-muted-foreground truncate font-medium">
              <Trans>
                Manage specialized extraction and identification options.
              </Trans>
            </p>
          </div>
        </div>

        {hasSpecificFields && (
          <div className="flex items-center p-1 bg-background/50 backdrop-blur-sm rounded-lg border border-border/50 shrink-0 self-start sm:self-auto">
            <Button
              type="button"
              variant={viewMode === 'form' ? 'secondary' : 'ghost'}
              size="sm"
              className={`h-8 px-4 gap-2 rounded-md transition-all shadow-none ${
                viewMode === 'form'
                  ? 'bg-background hover:bg-background border-border/50'
                  : ''
              }`}
              onClick={() => setViewMode('form')}
            >
              <List className="w-4 h-4" />
              <span className="text-xs font-bold uppercase tracking-wide">
                <Trans>Form</Trans>
              </span>
            </Button>
            <Button
              type="button"
              variant={viewMode === 'json' ? 'secondary' : 'ghost'}
              size="sm"
              className={`h-8 px-4 gap-2 rounded-md transition-all shadow-none ${
                viewMode === 'json'
                  ? 'bg-background hover:bg-background border-border/50'
                  : ''
              }`}
              onClick={() => setViewMode('json')}
            >
              <Code className="w-4 h-4" />
              <span className="text-xs font-bold uppercase tracking-wide">
                <Trans>JSON</Trans>
              </span>
            </Button>
          </div>
        )}
      </div>

      <CardContent className="p-0">
        <div className="animate-in fade-in slide-in-from-bottom-2 duration-300">
          {viewMode === 'form' ? (
            <div className="p-6 md:p-8">
              {renderPlatformFields() || (
                <div className="text-center py-16 text-muted-foreground border-2 border-dashed rounded-2xl bg-muted/20">
                  <Trans>
                    No specialized options available for this platform.
                  </Trans>
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
                  <div className="p-6 md:p-8 space-y-4">
                    <div className="flex items-center gap-2 text-indigo-500">
                      <Code className="w-4 h-4" />
                      <span className="text-sm font-bold uppercase tracking-wider">
                        <Trans>Raw JSON Editor</Trans>
                      </span>
                    </div>
                    <FormItem>
                      <FormControl>
                        <div className="space-y-2">
                          <Textarea
                            value={text}
                            onChange={handleChange}
                            className="font-mono text-sm min-h-[500px] bg-background/50 focus:bg-background border-border/50 focus-visible:ring-indigo-500 rounded-2xl shadow-inner scrollbar-none"
                            placeholder="{ ... }"
                          />
                          {error && (
                            <div className="p-3 rounded-lg bg-destructive/10 border border-destructive/20 text-xs font-semibold text-destructive animate-in shake duration-300">
                              Invalid JSON: {error}
                            </div>
                          )}
                        </div>
                      </FormControl>
                      <FormDescription className="text-xs font-medium">
                        <Trans>
                          Expert mode: Edit the raw platform-specific
                          configuration directly.
                        </Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  </div>
                );
              }}
            />
          )}
        </div>
      </CardContent>
    </Card>
  );
}
