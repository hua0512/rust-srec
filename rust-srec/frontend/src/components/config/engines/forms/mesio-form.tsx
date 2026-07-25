import React from 'react';
import { useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Badge } from '@/components/ui/badge';
import {
  Database,
  Wrench,
  Film,
  Settings2,
  RefreshCw,
  Layers,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { MesioHlsForm } from './mesio-hls-form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { cn } from '@/lib/utils';

interface SubFormProps {
  basePath: string;
}

const MesioFlvForm = React.memo(({ basePath }: SubFormProps) => {
  const { i18n } = useLingui();
  const duplicateTagFiltering = useWatch({
    name: `${basePath}.flv_fix.duplicate_tag_filtering`,
  });

  return (
    <div className="space-y-4">
      <Card className="border-border/50 shadow-sm">
        <CardContent className="p-4 space-y-6">
          {/* Header/Mode Section */}
          <div className="space-y-4">
            <FormField
              name={`${basePath}.flv_fix.sequence_header_change_mode`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel className="mb-2">
                    <RefreshCw className="w-3.5 h-3.5" />
                    <Trans>Stream Splitting Strategy</Trans>
                  </ConfigFieldLabel>
                  <Select
                    onValueChange={field.onChange}
                    defaultValue={field.value || 'crc32'}
                  >
                    <FormControl>
                      <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent className={CONFIG_SELECT_CONTENT}>
                      <SelectItem value="crc32" className="py-2.5">
                        <div className="flex flex-col gap-0.5">
                          <span className="font-medium text-xs">
                            crc32 (Default)
                          </span>
                          <span className="text-[10px] text-muted-foreground leading-relaxed max-w-[300px]">
                            <Trans>
                              Split on any raw header change. Safe but may cause
                              extra files.
                            </Trans>
                          </span>
                        </div>
                      </SelectItem>
                      <SelectItem value="semantic_signature" className="py-2.5">
                        <div className="flex flex-col gap-0.5">
                          <div className="flex items-center gap-2">
                            <span className="font-medium text-xs">
                              semantic_signature
                            </span>
                            <Badge
                              variant="secondary"
                              className="text-[8px] h-3.5 px-1 bg-orange-500/10 text-orange-600 border-none font-bold"
                            >
                              <Trans>NEW</Trans>
                            </Badge>
                          </div>
                          <span className="text-[10px] text-muted-foreground leading-relaxed max-w-[300px]">
                            <Trans>
                              Split only on codec configuration changes. Reduces
                              false splits.
                            </Trans>
                          </span>
                        </div>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="space-y-3">
            <FormField
              name={`${basePath}.flv_fix.drop_duplicate_sequence_headers`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-muted/5 p-4 py-3 shadow-none transition-all hover:bg-muted/10">
                  <div className="space-y-0.5">
                    <ConfigFieldLabel>
                      <Trans>Optimize Stream Headers</Trans>
                    </ConfigFieldLabel>
                    <FormDescription className={CONFIG_DESCRIPTION}>
                      <Trans>
                        Suppress redundant headers to reduce player
                        micro-stutter
                      </Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                      className="scale-90"
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            <FormField
              name={`${basePath}.flv_fix.duplicate_tag_filtering`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-muted/5 p-4 py-3 shadow-none transition-all hover:bg-muted/10">
                  <div className="space-y-0.5">
                    <ConfigFieldLabel>
                      <Layers className="w-3.5 h-3.5 text-blue-500" />
                      <Trans>Loop Protection</Trans>
                      <Badge
                        variant="secondary"
                        className="text-[8px] h-3.5 px-1 bg-blue-500/10 text-blue-600 border-none font-bold"
                      >
                        <Trans>BETA</Trans>
                      </Badge>
                    </ConfigFieldLabel>
                    <FormDescription className={CONFIG_DESCRIPTION}>
                      <Trans>
                        Filter repeated tags and detect stream replay loops
                      </Trans>
                    </FormDescription>
                  </div>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                      className="scale-90"
                    />
                  </FormControl>
                </FormItem>
              )}
            />

            {duplicateTagFiltering && (
              <div className="grid gap-3 pt-1">
                <div className="bg-blue-500/5 border border-blue-500/10 rounded-xl p-4 grid gap-4 sm:grid-cols-2">
                  <FormField
                    name={`${basePath}.flv_fix.duplicate_tag_filter_config.window_capacity_tags`}
                    render={({ field }) => (
                      <FormItem className="space-y-2">
                        <ConfigFieldLabel size="sm" className="mb-1">
                          <Trans>Filter Window Size</Trans>
                        </ConfigFieldLabel>
                        <FormControl>
                          <Input
                            type="number"
                            {...field}
                            className={cn(CONFIG_INPUT, 'font-mono')}
                            placeholder={i18n._(msg`Tags`)}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <FormField
                    name={`${basePath}.flv_fix.duplicate_tag_filter_config.replay_backjump_threshold_ms`}
                    render={({ field }) => (
                      <FormItem className="space-y-2">
                        <ConfigFieldLabel size="sm" className="mb-1">
                          <Trans>Backjump Threshold</Trans>
                        </ConfigFieldLabel>
                        <FormControl>
                          <Input
                            type="number"
                            {...field}
                            className={cn(CONFIG_INPUT, 'font-mono')}
                            placeholder="ms"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <div className="sm:col-span-2 pt-1 border-t border-blue-500/10">
                    <FormField
                      name={`${basePath}.flv_fix.duplicate_tag_filter_config.enable_replay_offset_matching`}
                      render={({ field }) => (
                        <FormItem className="flex flex-row items-center justify-between space-y-0">
                          <ConfigFieldLabel size="sm">
                            <Trans>Offset Consistency Check</Trans>
                          </ConfigFieldLabel>
                          <FormControl>
                            <Switch
                              checked={field.value}
                              onCheckedChange={field.onChange}
                              className="scale-75"
                            />
                          </FormControl>
                        </FormItem>
                      )}
                    />
                  </div>
                </div>
              </div>
            )}
          </div>
        </CardContent>
      </Card>
    </div>
  );
});
MesioFlvForm.displayName = 'MesioFlvForm';

interface MesioFormProps {
  basePath?: string;
}

export function MesioForm({ basePath = 'config' }: MesioFormProps) {
  const fixFlv = useWatch({
    name: `${basePath}.fix_flv`,
  });

  const fixHls = useWatch({
    name: `${basePath}.fix_hls`,
  });

  return (
    <Tabs defaultValue="general" className="w-full space-y-4">
      <TabsList className="w-full bg-background/40 border border-border/40 p-1 py-1.5 h-auto overflow-x-auto no-scrollbar justify-start">
        <TabsTrigger value="general" className="flex-1 min-w-[100px] gap-2">
          <Settings2 className="w-4 h-4 text-primary" />
          <Trans>General</Trans>
        </TabsTrigger>
        <TabsTrigger
          value="flv"
          className="flex-1 min-w-[100px] gap-2 disabled:opacity-50"
          disabled={!fixFlv}
        >
          <Film className="w-4 h-4 text-orange-500" />
          <Trans>FLV Tuning</Trans>
          {!fixFlv && (
            <Badge variant="outline" className="text-[8px] h-3 px-1 ml-1">
              Off
            </Badge>
          )}
        </TabsTrigger>
        <TabsTrigger
          value="hls"
          className="flex-1 min-w-[100px] gap-2 disabled:opacity-50"
          disabled={!fixHls}
        >
          <Wrench className="w-4 h-4 text-blue-500" />
          <Trans>HLS Tuning</Trans>
          {!fixHls && (
            <Badge variant="outline" className="text-[8px] h-3 px-1 ml-1">
              Off
            </Badge>
          )}
        </TabsTrigger>
      </TabsList>

      <TabsContent
        value="general"
        className="space-y-6 mt-0 focus-visible:outline-none"
      >
        <Card className="border-border/50 shadow-sm">
          <CardHeader className="pb-3 pt-4 px-4">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Database className="w-4 h-4 text-primary" />
              <Trans>Global Configuration</Trans>
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 pb-4">
            <FormField
              name={`${basePath}.buffer_size`}
              render={({ field }) => (
                <FormItem className="space-y-2">
                  <ConfigFieldLabel>
                    <Trans>Global Buffer Size</Trans>
                  </ConfigFieldLabel>
                  <FormControl>
                    <div className="flex items-center gap-2">
                      <Input
                        type="number"
                        {...field}
                        className="bg-background/50 font-mono"
                      />
                      <span className="text-xs text-muted-foreground whitespace-nowrap">
                        <Trans>bytes</Trans>
                      </span>
                    </div>
                  </FormControl>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Recommended: 8388608 (8 MiB)</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>

        <div className="grid gap-4 md:grid-cols-2">
          <FormField
            name={`${basePath}.fix_flv`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-gradient-to-br from-background/50 to-orange-500/5 p-4 shadow-sm transition-all hover:border-orange-500/20">
                <div className="space-y-0.5">
                  <ConfigFieldLabel>
                    <Film className="w-4 h-4 text-orange-500" />
                    <Trans>Fix FLV Streams</Trans>
                  </ConfigFieldLabel>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Enable advanced FLV timestamp repairing</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />
          <FormField
            name={`${basePath}.fix_hls`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-gradient-to-br from-background/50 to-blue-500/5 p-4 shadow-sm transition-all hover:border-blue-500/20">
                <div className="space-y-0.5">
                  <ConfigFieldLabel>
                    <Wrench className="w-4 h-4 text-blue-500" />
                    <Trans>Fix HLS Discontinuities</Trans>
                  </ConfigFieldLabel>
                  <FormDescription className={CONFIG_DESCRIPTION}>
                    <Trans>Enable advanced HLS segment reconstruction</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={field.value}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>
      </TabsContent>

      <TabsContent value="flv" className="mt-0 focus-visible:outline-none">
        <MesioFlvForm basePath={basePath} />
      </TabsContent>

      <TabsContent value="hls" className="mt-0 focus-visible:outline-none">
        <MesioHlsForm basePath={basePath} />
      </TabsContent>
    </Tabs>
  );
}
