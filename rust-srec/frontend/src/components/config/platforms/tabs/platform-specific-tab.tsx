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
import { useState, useEffect, useRef } from 'react';
import { Button } from '@/components/ui/button';
import {
  HuyaConfigSchema,
  DouyinConfigSchema,
  BilibiliConfigSchema,
  DouyuConfigSchema,
  TwitchConfigSchema,
  TikTokConfigSchema,
  TwitcastingConfigSchema,
  SoopConfigSchema,
  BigoConfigSchema,
} from '@/api/schemas';
import { HuyaConfigFields } from './specific-configs/huya-config-fields';
import { DouyinConfigFields } from './specific-configs/douyin-config-fields';
import { BilibiliConfigFields } from './specific-configs/bilibili-config-fields';
import { DouyuConfigFields } from './specific-configs/douyu-config-fields';
import { TwitchConfigFields } from './specific-configs/twitch-config-fields';
import { TikTokConfigFields } from './specific-configs/tiktok-config-fields';
import { TwitcastingConfigFields } from './specific-configs/twitcasting-config-fields';
import { SoopConfigFields } from './specific-configs/soop-config-fields';
import { BigoConfigFields } from './specific-configs/bigo-config-fields';

const PLATFORM_SCHEMAS: Record<string, any> = {
  huya: HuyaConfigSchema,
  douyin: DouyinConfigSchema,
  bilibili: BilibiliConfigSchema,
  douyu: DouyuConfigSchema,
  twitch: TwitchConfigSchema,
  tiktok: TikTokConfigSchema,
  twitcasting: TwitcastingConfigSchema,
  soop: SoopConfigSchema,
  bigo: BigoConfigSchema,
};

const SPECIFIC_CONFIG_COMPONENTS: Record<string, any> = {
  huya: HuyaConfigFields,
  douyin: DouyinConfigFields,
  bilibili: BilibiliConfigFields,
  douyu: DouyuConfigFields,
  twitch: TwitchConfigFields,
  tiktok: TikTokConfigFields,
  twitcasting: TwitcastingConfigFields,
  soop: SoopConfigFields,
  bigo: BigoConfigFields,
};

function toJsonText(value: unknown): string {
  if (value === null || value === undefined) return '';
  // A stored string is already the raw text; anything else is printed as JSON.
  if (typeof value === 'string') return value;
  return JSON.stringify(value, null, 2) ?? '';
}

/**
 * Identity of a form value for change detection.
 *
 * The form hands back a fresh deep copy on every update, so references say
 * nothing about whether the options actually changed.
 */
function identityOf(value: unknown): string {
  return value === undefined ? '' : (JSON.stringify(value) ?? '');
}

/**
 * Raw JSON view of the platform options.
 *
 * The textarea owns its text. Re-deriving it from the parsed form value would
 * reprint what is being typed as indented JSON and send the caret to the end, so
 * the form value is only adopted when it differs from what this editor last
 * emitted — a config load or a form reset, never a keystroke.
 */
function RawJsonEditor({
  form,
  fieldName,
  platformName,
  value,
  onChange,
}: {
  form: UseFormReturn<any>;
  fieldName: string;
  platformName?: string;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const [text, setText] = useState(() => toJsonText(value));
  const [error, setError] = useState<string | null>(null);
  const lastEmitted = useRef(identityOf(value));

  useEffect(() => {
    const identity = identityOf(value);
    if (identity === lastEmitted.current) return;
    lastEmitted.current = identity;
    setText(toJsonText(value));
    setError(null);
  }, [value]);

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const newText = e.target.value;
    setText(newText);

    const emit = (parsed: unknown) => {
      lastEmitted.current = identityOf(parsed);
      onChange(parsed);
      setError(null);
      form.clearErrors(fieldName);
    };

    if (!newText.trim()) {
      emit(null);
      return;
    }

    try {
      const parsed = JSON.parse(newText);
      const schema = platformName
        ? PLATFORM_SCHEMAS[platformName.toLowerCase()]
        : null;
      if (schema) {
        schema.parse(parsed);
      }
      emit(parsed);
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
                <Trans>Invalid JSON: {error}</Trans>
              </div>
            )}
          </div>
        </FormControl>
        <FormDescription className="text-xs font-medium">
          <Trans>
            Expert mode: Edit the raw platform-specific configuration directly.
          </Trans>
        </FormDescription>
        <FormMessage />
      </FormItem>
    </div>
  );
}

interface PlatformSpecificTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
  platformName?: string;
  /**
   * Field under `basePath` holding the options. Defaults to `platform_specific_config`, which is
   * what the platform and template rows use; a streamer stores the same shape under
   * `platform_extras`, which is the key its config resolver reads.
   */
  field?: string;
  inherited?: boolean;
}

export function PlatformSpecificTab({
  form,
  basePath,
  platformName,
  field: fieldKey = 'platform_specific_config',
  inherited = false,
}: PlatformSpecificTabProps) {
  const fieldName = basePath ? `${basePath}.${fieldKey}` : fieldKey;

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

    return (
      <SpecificFields form={form} fieldName={fieldName} inherited={inherited} />
    );
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
              render={({ field }) => (
                <RawJsonEditor
                  form={form}
                  fieldName={fieldName}
                  platformName={platformName}
                  value={field.value}
                  onChange={field.onChange}
                />
              )}
            />
          )}
        </div>
      </CardContent>
    </Card>
  );
}
