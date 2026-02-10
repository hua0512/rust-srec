import React from 'react';
import { Control } from 'react-hook-form';
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Trans } from '@lingui/react/macro';
import { Card, CardContent } from '@/components/ui/card';
import { Globe, ListMusic, Bot, Zap, Share2 } from 'lucide-react';

interface SubFormProps {
  control: Control<any>;
  hlsPath: string;
}

const HlsBaseSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.base.timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Global Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 0 (No timeout)"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.connect_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Connect Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.read_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Read Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.write_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Write Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.base.user_agent`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>User Agent</Trans>
            </FormLabel>
            <FormControl>
              <Input
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: Mozilla/5.0..."
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.http_version`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>HTTP Version Preference</Trans>
            </FormLabel>
            <Select
              onValueChange={field.onChange}
              defaultValue={field.value || 'auto'}
            >
              <FormControl>
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue placeholder="Auto" />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem value="auto">
                  <Trans>Auto (Default)</Trans>
                </SelectItem>
                <SelectItem value="http2_only">
                  <Trans>HTTP/2 Only</Trans>
                </SelectItem>
                <SelectItem value="http1_only">
                  <Trans>HTTP/1.1 Only</Trans>
                </SelectItem>
              </SelectContent>
            </Select>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="grid gap-4 sm:grid-cols-3">
      <FormField
        control={control}
        name={`${hlsPath}.base.http2_keep_alive_interval_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>H2 Keep-Alive (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 20000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.pool_max_idle_per_host`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Idle per Host</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.pool_idle_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Pool Idle Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="grid gap-2 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.base.follow_redirects`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal">
              <Trans>Follow Redirects (Default: On)</Trans>
            </FormLabel>
            <FormControl>
              <Switch
                checked={field.value ?? true}
                onCheckedChange={field.onChange}
                className="scale-75 origin-right"
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.danger_accept_invalid_certs`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal text-destructive/80">
              <Trans>Accept Invalid Certs (Default: Off)</Trans>
            </FormLabel>
            <FormControl>
              <Switch
                checked={field.value ?? false}
                onCheckedChange={field.onChange}
                className="scale-75 origin-right"
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.force_ipv4`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal">
              <Trans>Force IPv4 (Default: Off)</Trans>
            </FormLabel>
            <FormControl>
              <Switch
                checked={field.value ?? false}
                onCheckedChange={field.onChange}
                className="scale-75 origin-right"
              />
            </FormControl>
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.base.force_ipv6`}
        render={({ field }) => (
          <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 px-3 py-2 shadow-sm">
            <FormLabel className="text-[11px] font-normal">
              <Trans>Force IPv6 (Default: Off)</Trans>
            </FormLabel>
            <FormControl>
              <Switch
                checked={field.value ?? false}
                onCheckedChange={field.onChange}
                className="scale-75 origin-right"
              />
            </FormControl>
          </FormItem>
        )}
      />
    </div>
  </div>
));
HlsBaseSettings.displayName = 'HlsBaseSettings';

const HlsPlaylistSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.initial_playlist_fetch_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Initial Fetch Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 15000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.live_refresh_interval_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Live Refresh Interval (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 1000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.live_max_refresh_retries`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Refresh Retries</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 5"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.playlist_config.live_refresh_retry_delay_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Retry Delay (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 1000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <Card className="border-border/40 bg-muted/5">
      <CardContent className="p-3 space-y-3">
        <FormField
          control={control}
          name={`${hlsPath}.playlist_config.adaptive_refresh_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between">
              <div className="space-y-0.5">
                <FormLabel className="text-xs font-medium">
                  <Trans>Adaptive Refresh (Default: On)</Trans>
                </FormLabel>
                <FormDescription className="text-[10px]">
                  <Trans>Adjust rate based on target duration</Trans>
                </FormDescription>
              </div>
              <FormControl>
                <Switch
                  checked={field.value ?? true}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />

        <div className="grid gap-3 sm:grid-cols-2 pt-2 border-t border-border/40">
          <FormField
            control={control}
            name={`${hlsPath}.playlist_config.adaptive_refresh_min_interval_ms`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Min Interval (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-7 text-xs font-mono"
                    placeholder="Default: 500"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name={`${hlsPath}.playlist_config.adaptive_refresh_max_interval_ms`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px] text-muted-foreground">
                  <Trans>Max Interval (ms)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-7 text-xs font-mono"
                    placeholder="Default: 3000"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
      </CardContent>
    </Card>
  </div>
));
HlsPlaylistSettings.displayName = 'HlsPlaylistSettings';

const HlsFetcherSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.segment_download_timeout_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Segment Timeout (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.max_segment_retries`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Segment Retries</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 3"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.segment_retry_delay_base_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Retry Delay Base (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 500"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.streaming_threshold_bytes`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Streaming Threshold (bytes)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 2097152 (2 MiB)"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="space-y-2">
      <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        <Trans>Decryption Keys</Trans>
      </h4>
      <div className="grid gap-4 sm:grid-cols-3">
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.key_download_timeout_ms`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Timeout (ms)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 5000"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.max_key_retries`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Retries</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 3"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.fetcher_config.key_retry_delay_base_ms`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Retry Delay (ms)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 200"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>

    <div className="space-y-2">
      <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
        <Trans>Caching</Trans>
      </h4>
      <FormField
        control={control}
        name={`${hlsPath}.fetcher_config.segment_raw_cache_ttl_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-[10px]">
              <Trans>Raw Segment TTL (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 60000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>
  </div>
));
HlsFetcherSettings.displayName = 'HlsFetcherSettings';

const HlsPerformanceSettings = React.memo(
  ({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
      <div className="grid gap-2">
        <FormField
          control={control}
          name={`${hlsPath}.performance_config.zero_copy_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
              <FormLabel className="text-xs font-normal">
                <Trans>Zero Copy Processing (Default: On)</Trans>
              </FormLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? true}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.performance_config.decryption_offload_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
              <FormLabel className="text-xs font-normal">
                <Trans>Offload Decryption (Default: On)</Trans>
              </FormLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? true}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.performance_config.metrics_enabled`}
          render={({ field }) => (
            <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
              <FormLabel className="text-xs font-normal">
                <Trans>Enable Performance Metrics (Default: On)</Trans>
              </FormLabel>
              <FormControl>
                <Switch
                  checked={field.value ?? true}
                  onCheckedChange={field.onChange}
                  className="scale-75 origin-right"
                />
              </FormControl>
            </FormItem>
          )}
        />
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          control={control}
          name={`${hlsPath}.download_concurrency`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-xs font-semibold">
                <Trans>Download Concurrency</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 5"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>

      <div className="space-y-3">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground border-b border-border/40 pb-1">
          <Trans>Prefetching</Trans>
        </h4>
        <div className="flex gap-4">
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.prefetch.enabled`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center gap-2 space-y-0">
                <FormControl>
                  <Switch
                    checked={field.value ?? false}
                    onCheckedChange={field.onChange}
                    className="scale-75"
                  />
                </FormControl>
                <FormLabel className="text-xs font-normal">
                  <Trans>Enabled (Default: Off)</Trans>
                </FormLabel>
              </FormItem>
            )}
          />
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.prefetch.prefetch_count`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px]">
                  <Trans>Prefetch Count</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
                    placeholder="Default: 2"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={control}
            name={`${hlsPath}.performance_config.prefetch.max_buffer_before_skip`}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-[10px]">
                  <Trans>Max Buffer Before Skip</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    {...field}
                    className="h-8 text-xs font-mono"
                    placeholder="Default: 40"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
      </div>
    </div>
  ),
);
HlsPerformanceSettings.displayName = 'HlsPerformanceSettings';

const HlsOutputSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
  <div className="space-y-4 animate-in fade-in duration-300">
    <div className="grid gap-4 sm:grid-cols-2">
      <FormField
        control={control}
        name={`${hlsPath}.output_config.live_reorder_buffer_duration_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Reorder Duration (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 30000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.output_config.live_reorder_buffer_max_segments`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Reorder Max Segments</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 10"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.output_config.gap_evaluation_interval_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Gap Eval Interval (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 200"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
      <FormField
        control={control}
        name={`${hlsPath}.output_config.live_max_overall_stall_duration_ms`}
        render={({ field }) => (
          <FormItem>
            <FormLabel className="text-xs">
              <Trans>Max Stall Duration (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                {...field}
                className="h-8 text-xs font-mono"
                placeholder="Default: 60000"
              />
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />
    </div>

    <div className="space-y-3">
      <h4 className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground border-b border-border/40 pb-1">
        <Trans>Buffer Limits</Trans>
      </h4>
      <div className="grid gap-4 sm:grid-cols-2">
        <FormField
          control={control}
          name={`${hlsPath}.output_config.buffer_limits.max_segments`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Segments</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 50"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={control}
          name={`${hlsPath}.output_config.buffer_limits.max_bytes`}
          render={({ field }) => (
            <FormItem>
              <FormLabel className="text-[10px]">
                <Trans>Max Bytes</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  {...field}
                  className="h-8 text-xs font-mono"
                  placeholder="Default: 104857600 (100 MiB)"
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
      </div>
    </div>
  </div>
));
HlsOutputSettings.displayName = 'HlsOutputSettings';

interface MesioHlsFormProps {
  control: Control<any>;
  basePath?: string;
}

export function MesioHlsForm({
  control,
  basePath = 'config',
}: MesioHlsFormProps) {
  const hlsPath = `${basePath}.hls`;

  return (
    <Card className="border-border/40 bg-background/20 shadow-none overflow-hidden animate-in fade-in slide-in-from-top-1 duration-200">
      <CardContent className="p-3">
        <Tabs defaultValue="base" className="w-full">
          <TabsList className="flex w-full mb-4 bg-muted/30 p-1 py-1.5 h-auto overflow-x-auto no-scrollbar justify-start">
            <TabsTrigger
              value="base"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Globe className="w-3 h-3 text-sky-500" />
              <span className="hidden sm:inline">
                <Trans>Base</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="playlist"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <ListMusic className="w-3 h-3 text-pink-500" />
              <span className="hidden sm:inline">
                <Trans>Playlist</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="fetcher"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Bot className="w-3 h-3 text-purple-500" />
              <span className="hidden sm:inline">
                <Trans>Fetcher</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="performance"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Zap className="w-3 h-3 text-yellow-500" />
              <span className="hidden sm:inline">
                <Trans>Perf</Trans>
              </span>
            </TabsTrigger>
            <TabsTrigger
              value="output"
              className="flex-1 min-w-[60px] text-[10px] gap-1 px-1"
            >
              <Share2 className="w-3 h-3 text-emerald-500" />
              <span className="hidden sm:inline">
                <Trans>Output</Trans>
              </span>
            </TabsTrigger>
          </TabsList>

          <TabsContent value="base" className="mt-0">
            <HlsBaseSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="playlist" className="mt-0">
            <HlsPlaylistSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="fetcher" className="mt-0">
            <HlsFetcherSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="performance" className="mt-0">
            <HlsPerformanceSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
          <TabsContent value="output" className="mt-0">
            <HlsOutputSettings control={control} hlsPath={hlsPath} />
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  );
}
