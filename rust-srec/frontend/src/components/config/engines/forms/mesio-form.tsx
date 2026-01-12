import React from 'react';
import { Control, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
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

interface SubFormProps {
  control: Control<any>;
  basePath: string;
}

const MesioFlvForm = React.memo(({ control, basePath }: SubFormProps) => {
  const duplicateTagFiltering = useWatch({
    control,
    name: `${basePath}.flv_fix.duplicate_tag_filtering`,
  });

  return (
    <div className="space-y-4 animate-in fade-in slide-in-from-top-2 duration-300">
      <Card className="border-border/40 bg-background/20 shadow-none overflow-hidden">
        <CardContent className="p-4 space-y-6">
          {/* Header/Mode Section */}
          <div className="space-y-4">
            <FormField
              control={control}
              name={`${basePath}.flv_fix.sequence_header_change_mode`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs font-semibold flex items-center gap-2 mb-2 text-orange-500/80 uppercase tracking-tighter">
                    <RefreshCw className="w-3.5 h-3.5" />
                    <Trans>Stream Splitting Strategy</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    defaultValue={field.value || 'crc32'}
                  >
                    <FormControl>
                      <SelectTrigger className="bg-background/50 border-border/40 h-10 transition-all hover:border-orange-500/30">
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
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
              control={control}
              name={`${basePath}.flv_fix.drop_duplicate_sequence_headers`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-muted/5 p-4 py-3 shadow-none transition-all hover:bg-muted/10">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs font-medium">
                      <Trans>Optimize Stream Headers</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
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
              control={control}
              name={`${basePath}.flv_fix.duplicate_tag_filtering`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-muted/5 p-4 py-3 shadow-none transition-all hover:bg-muted/10">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs font-medium flex items-center gap-2">
                      <Layers className="w-3.5 h-3.5 text-blue-500" />
                      <Trans>Loop Protection</Trans>
                      <Badge
                        variant="secondary"
                        className="text-[8px] h-3.5 px-1 bg-blue-500/10 text-blue-600 border-none font-bold"
                      >
                        <Trans>BETA</Trans>
                      </Badge>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
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
              <div className="grid gap-3 pt-1 animate-in fade-in slide-in-from-left-2 duration-300">
                <div className="bg-blue-500/5 border border-blue-500/10 rounded-xl p-4 grid gap-4 sm:grid-cols-2">
                  <FormField
                    control={control}
                    name={`${basePath}.flv_fix.duplicate_tag_filter_config.window_capacity_tags`}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-[10px] font-semibold text-blue-500/80 uppercase tracking-tight mb-1">
                          <Trans>Filter Window Size</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            type="number"
                            {...field}
                            className="h-8 text-xs bg-background/50 border-blue-500/20 focus-visible:ring-blue-500/30 font-mono"
                            placeholder="Tags"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <FormField
                    control={control}
                    name={`${basePath}.flv_fix.duplicate_tag_filter_config.replay_backjump_threshold_ms`}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-[10px] font-semibold text-blue-500/80 uppercase tracking-tight mb-1">
                          <Trans>Backjump Threshold</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            type="number"
                            {...field}
                            className="h-8 text-xs bg-background/50 border-blue-500/20 focus-visible:ring-blue-500/30 font-mono"
                            placeholder="ms"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <div className="sm:col-span-2 pt-1 border-t border-blue-500/10">
                    <FormField
                      control={control}
                      name={`${basePath}.flv_fix.duplicate_tag_filter_config.enable_replay_offset_matching`}
                      render={({ field }) => (
                        <FormItem className="flex flex-row items-center justify-between space-y-0">
                          <FormLabel className="text-[10px] font-medium text-blue-500/80 uppercase tracking-tight">
                            <Trans>Offset Consistency Check</Trans>
                          </FormLabel>
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
  control: Control<any>;
  basePath?: string;
}

export function MesioForm({ control, basePath = 'config' }: MesioFormProps) {
  const fixFlv = useWatch({
    control,
    name: `${basePath}.fix_flv`,
  });

  const fixHls = useWatch({
    control,
    name: `${basePath}.fix_hls`,
  });

  return (
    <Tabs defaultValue="general" className="w-full space-y-4">
      <TabsList className="bg-background/40 border border-border/40 p-1 h-11 shrink-0">
        <TabsTrigger value="general" className="flex-1 gap-2">
          <Settings2 className="w-4 h-4 text-primary" />
          <Trans>General</Trans>
        </TabsTrigger>
        <TabsTrigger
          value="flv"
          className="flex-1 gap-2 disabled:opacity-50"
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
          className="flex-1 gap-2 disabled:opacity-50"
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
        <Card className="border-border/40 bg-background/40 shadow-sm">
          <CardHeader className="pb-3 pt-4 px-4">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Database className="w-4 h-4 text-primary" />
              <Trans>Global Configuration</Trans>
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 pb-4">
            <FormField
              control={control}
              name={`${basePath}.buffer_size`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                    <Trans>Global Buffer Size</Trans>
                  </FormLabel>
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
                  <FormDescription className="text-[10px]">
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
            control={control}
            name={`${basePath}.fix_flv`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-gradient-to-br from-background/50 to-orange-500/5 p-4 shadow-sm transition-all hover:border-orange-500/20">
                <div className="space-y-0.5">
                  <FormLabel className="text-sm font-medium flex items-center gap-2">
                    <Film className="w-4 h-4 text-orange-500" />
                    <Trans>Fix FLV Streams</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px]">
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
            control={control}
            name={`${basePath}.fix_hls`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-gradient-to-br from-background/50 to-blue-500/5 p-4 shadow-sm transition-all hover:border-blue-500/20">
                <div className="space-y-0.5">
                  <FormLabel className="text-sm font-medium flex items-center gap-2">
                    <Wrench className="w-4 h-4 text-blue-500" />
                    <Trans>Fix HLS Discontinuities</Trans>
                  </FormLabel>
                  <FormDescription className="text-[10px]">
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
        <MesioFlvForm control={control} basePath={basePath} />
      </TabsContent>

      <TabsContent value="hls" className="mt-0 focus-visible:outline-none">
        <MesioHlsForm control={control} basePath={basePath} />
      </TabsContent>
    </Tabs>
  );
}
