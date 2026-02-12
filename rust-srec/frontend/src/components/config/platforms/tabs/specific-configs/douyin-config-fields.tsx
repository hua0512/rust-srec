import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Trans } from '@lingui/react/macro';
import {
  Zap,
  Cookie,
  Shield,
  Smartphone,
  Gamepad2,
  Monitor,
} from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { EndStreamOnDanmuCloseField } from '@/components/config/shared/end-stream-on-danmu-close-field';

interface DouyinConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function DouyinConfigFields({
  form,
  fieldName,
}: DouyinConfigFieldsProps) {
  return (
    <div className="space-y-12">
      {/* Extraction Settings Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Zap className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Extraction Settings</Trans>
          </h4>
        </div>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.force_origin_quality`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-2xl border bg-muted/5 p-5 transition-all hover:bg-muted/10 border-border/50">
                <div className="space-y-1.5 pr-4">
                  <div className="flex items-center gap-2">
                    <FormLabel className="text-sm font-bold text-foreground">
                      <Trans>Force Origin Quality</Trans>
                    </FormLabel>
                    <Badge
                      variant="secondary"
                      className="text-[10px] h-4 bg-orange-500/10 text-orange-600 border-orange-200 dark:border-orange-500/20 px-1.5 font-bold uppercase tracking-wider"
                    >
                      <Trans>Experimental</Trans>
                    </Badge>
                  </div>
                  <FormDescription className="text-xs leading-relaxed font-medium">
                    <Trans>
                      Attempts to get origin quality by replacing the audio
                      stream. May result in audio-only streams if it fails.
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <FormField
              control={form.control}
              name={`${fieldName}.double_screen`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                  <div className="flex items-center gap-3">
                    <div className="p-2 rounded-lg bg-muted text-muted-foreground shrink-0 leading-none">
                      <Monitor className="w-4 h-4" />
                    </div>
                    <div className="space-y-0.5">
                      <FormLabel className="text-xs font-bold text-foreground">
                        <Trans>Double Screen Data</Trans>
                      </FormLabel>
                      <FormDescription className="text-[10px] leading-tight font-medium">
                        <Trans>Capture separate stream data.</Trans>
                      </FormDescription>
                    </div>
                  </div>
                  <FormControl>
                    <Switch
                      checked={!!field.value}
                      onCheckedChange={field.onChange}
                      defaultValue={field.value || true}
                      className="scale-90"
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              control={form.control}
              name={`${fieldName}.force_mobile_api`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                  <div className="flex items-center gap-3">
                    <div className="p-2 rounded-lg bg-muted text-muted-foreground shrink-0 leading-none">
                      <Smartphone className="w-4 h-4" />
                    </div>
                    <div className="space-y-0.5">
                      <FormLabel className="text-xs font-bold text-foreground">
                        <Trans>Force Mobile API</Trans>
                      </FormLabel>
                      <FormDescription className="text-[10px] leading-tight font-medium">
                        <Trans>Use mobile endpoint for extraction.</Trans>
                      </FormDescription>
                    </div>
                  </div>
                  <FormControl>
                    <Switch
                      checked={!!field.value}
                      onCheckedChange={field.onChange}
                      className="scale-90"
                    />
                  </FormControl>
                </FormItem>
              )}
            />
          </div>

          <FormField
            control={form.control}
            name={`${fieldName}.skip_interactive_games`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 bg-background/50 transition-colors hover:bg-muted/5">
                <div className="flex items-center gap-3">
                  <div className="p-2 rounded-lg bg-muted text-muted-foreground shrink-0 leading-none">
                    <Gamepad2 className="w-4 h-4" />
                  </div>
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs font-bold text-foreground">
                      <Trans>Skip Interactive Games</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px] leading-tight font-medium">
                      <Trans>
                        Treat interactive game streams (互动玩法) as offline.
                      </Trans>
                    </FormDescription>
                  </div>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                    defaultValue={field.value || true}
                    className="scale-90"
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>
      </section>

      {/* Security & Identity Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Shield className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Security & Identity</Trans>
          </h4>
        </div>

        <div className="space-y-6">
          <FormField
            control={form.control}
            name={`${fieldName}.ttwid_management_mode`}
            render={({ field }) => (
              <FormItem>
                <div className="flex items-center gap-2 mb-3">
                  <div className="w-1.5 h-1.5 rounded-full bg-indigo-500" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>TTWID Management Mode</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Tabs
                    onValueChange={field.onChange}
                    value={field.value || 'global'}
                    className="w-full"
                  >
                    <TabsList className="grid w-full grid-cols-2 bg-muted/30 p-1 rounded-xl h-11 border border-border/40">
                      <TabsTrigger
                        value="global"
                        className="rounded-lg data-[state=active]:bg-background data-[state=active]:shadow-sm font-bold text-xs uppercase tracking-wide"
                      >
                        <Trans>Global</Trans>
                      </TabsTrigger>
                      <TabsTrigger
                        value="per-streamer"
                        className="rounded-lg data-[state=active]:bg-background data-[state=active]:shadow-sm font-bold text-xs uppercase tracking-wide"
                      >
                        <Trans>Per-Streamer</Trans>
                      </TabsTrigger>
                    </TabsList>
                  </Tabs>
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    Isolation level for tracking identifiers. Global is usually
                    best.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={`${fieldName}.ttwid`}
            render={({ field }) => (
              <FormItem>
                <div className="flex items-center gap-2 mb-3">
                  <div className="p-1.5 rounded-md bg-muted text-muted-foreground">
                    <Cookie className="w-3.5 h-3.5" />
                  </div>
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>Specific TTWID Cookie</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Input
                    placeholder="ttwid=..."
                    {...field}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs"
                  />
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    Explicit TTWID cookie value to use for all requests.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>

      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Shield className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Danmu Control</Trans>
          </h4>
        </div>

        <EndStreamOnDanmuCloseField
          form={form}
          name={`${fieldName}.end_stream_on_danmu_stream_closed`}
        />
      </section>
    </div>
  );
}
