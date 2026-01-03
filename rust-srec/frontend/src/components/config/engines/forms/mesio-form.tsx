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
  FlaskConical,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';

interface MesioFormProps {
  control: Control<any>;
  basePath?: string;
}

export function MesioForm({ control, basePath = 'config' }: MesioFormProps) {
  const fixFlv = useWatch({
    control,
    name: `${basePath}.fix_flv`,
  });

  return (
    <div className="space-y-6">
      <Card className="border-border/40 bg-background/40 shadow-sm">
        <CardHeader className="pb-3 pt-4 px-4">
          <CardTitle className="text-sm font-medium flex items-center gap-2">
            <Database className="w-4 h-4 text-primary" />
            <Trans>Buffer Settings</Trans>
          </CardTitle>
        </CardHeader>
        <CardContent className="px-4 pb-4">
          <FormField
            control={control}
            name={`${basePath}.buffer_size`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                  <Trans>Buffer Size</Trans>
                </FormLabel>
                <FormControl>
                  <div className="flex items-center gap-2">
                    <Input
                      type="number"
                      {...field}
                      className="bg-background/50"
                    />
                    <span className="text-xs text-muted-foreground whitespace-nowrap">
                      <Trans>bytes</Trans>
                    </span>
                  </div>
                </FormControl>
                <FormDescription className="text-[10px]">
                  <Trans>Default: 8388608 (8 MiB)</Trans>
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
            <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-background/40 p-4 shadow-sm transition-colors">
              <div className="space-y-0.5">
                <FormLabel className="text-sm font-medium flex items-center gap-2">
                  <Film className="w-4 h-4 text-orange-500" />
                  <Trans>Fix FLV</Trans>
                </FormLabel>
                <FormDescription className="text-[10px]">
                  <Trans>Attempt to repair timestamps in FLV streams</Trans>
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
            <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 bg-background/40 p-4 shadow-sm transition-colors">
              <div className="space-y-0.5">
                <FormLabel className="text-sm font-medium flex items-center gap-2">
                  <Wrench className="w-4 h-4 text-blue-500" />
                  <Trans>Fix HLS</Trans>
                </FormLabel>
                <FormDescription className="text-[10px]">
                  <Trans>Handle discontinuities in HLS streams</Trans>
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

      {fixFlv && (
        <Card className="border-border/40 bg-background/40 shadow-sm overflow-hidden animate-in fade-in slide-in-from-top-2 duration-300">
          <CardHeader className="pb-3 pt-4 px-4 bg-muted/20 border-b border-border/40">
            <CardTitle className="text-sm font-medium flex items-center gap-2">
              <Settings2 className="w-4 h-4 text-primary" />
              <Trans>FLV Fix (Advanced)</Trans>
            </CardTitle>
          </CardHeader>
          <CardContent className="px-4 py-4 space-y-6">
            <FormField
              control={control}
              name={`${basePath}.flv_fix.sequence_header_change_mode`}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold flex items-center gap-2">
                    <RefreshCw className="w-3.5 h-3.5" />
                    <Trans>Split on sequence header change</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    defaultValue={field.value || 'crc32'}
                  >
                    <FormControl>
                      <SelectTrigger className="bg-background/50">
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="crc32">
                        <div className="flex flex-col gap-0.5">
                          <span className="font-medium text-xs">
                            crc32 (legacy)
                          </span>
                          <span className="text-[10px] text-muted-foreground">
                            <Trans>
                              Split whenever raw sequence-header bytes change
                              (most conservative, may split on non-semantic
                              changes).
                            </Trans>
                          </span>
                        </div>
                      </SelectItem>
                      <SelectItem value="semantic_signature">
                        <div className="flex flex-col gap-0.5">
                          <div className="flex items-center gap-2">
                            <span className="font-medium text-xs">
                              semantic_signature
                            </span>
                            <Badge
                              variant="secondary"
                              className="text-[8px] h-3.5 px-1 bg-amber-500/10 text-amber-600 border-amber-500/20"
                            >
                              <FlaskConical className="w-2 h-2 mr-0.5" />
                              <Trans>Experimental</Trans>
                            </Badge>
                          </div>
                          <span className="text-[10px] text-muted-foreground">
                            <Trans>
                              Split only when codec configuration changes
                              (reduces false splits).
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

            <FormField
              control={control}
              name={`${basePath}.flv_fix.drop_duplicate_sequence_headers`}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between space-y-0">
                  <div className="space-y-0.5">
                    <FormLabel className="text-xs font-semibold">
                      <Trans>Drop duplicate sequence headers</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>
                        Suppress redundant sequence headers with the same config
                        (can reduce player stutter).
                      </Trans>
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

            <div className="space-y-4 pt-2 border-t border-border/40">
              <FormField
                control={control}
                name={`${basePath}.flv_fix.duplicate_tag_filtering`}
                render={({ field }) => (
                  <FormItem className="flex flex-row items-center justify-between space-y-0">
                    <div className="space-y-0.5">
                      <FormLabel className="text-xs font-semibold flex items-center gap-2">
                        <Layers className="w-3.5 h-3.5 text-blue-500" />
                        <Trans>Duplicate media-tag filtering</Trans>
                        <Badge
                          variant="secondary"
                          className="text-[8px] h-3.5 px-1 bg-amber-500/10 text-amber-600 border-amber-500/20"
                        >
                          <FlaskConical className="w-2 h-2 mr-0.5" />
                          <Trans>Experimental</Trans>
                        </Badge>
                      </FormLabel>
                      <FormDescription className="text-[10px]">
                        <Trans>
                          Drop repeated A/V tags and detect replay loops (useful
                          when streams go offline and replay tail).
                        </Trans>
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

              {useWatch({
                control,
                name: `${basePath}.flv_fix.duplicate_tag_filtering`,
              }) && (
                <div className="grid gap-4 md:grid-cols-2 pl-4 border-l-2 border-blue-500/20 animate-in fade-in slide-in-from-left-2 duration-200">
                  <FormField
                    control={control}
                    name={`${basePath}.flv_fix.duplicate_tag_filter_config.window_capacity_tags`}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-[10px] uppercase tracking-wider text-muted-foreground font-semibold">
                          <Trans>Window Capacity (Tags)</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            type="number"
                            {...field}
                            className="h-8 text-xs bg-background/50"
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
                        <FormLabel className="text-[10px] uppercase tracking-wider text-muted-foreground font-semibold">
                          <Trans>Replay Backjump Threshold (ms)</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            type="number"
                            {...field}
                            className="h-8 text-xs bg-background/50"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <div className="md:col-span-2">
                    <FormField
                      control={control}
                      name={`${basePath}.flv_fix.duplicate_tag_filter_config.enable_replay_offset_matching`}
                      render={({ field }) => (
                        <FormItem className="flex flex-row items-center justify-between space-y-0">
                          <FormLabel className="text-[10px] font-semibold">
                            <Trans>Enable Replay Offset Matching</Trans>
                          </FormLabel>
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
                </div>
              )}
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
