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
import {
    Tabs,
    TabsContent,
    TabsList,
    TabsTrigger,
} from '@/components/ui/tabs';
import { Trans } from '@lingui/react/macro';
import { Card, CardContent } from '@/components/ui/card';
import {
    Globe,
    ListMusic,
    Bot,
    Zap,
} from 'lucide-react';

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
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: system" />
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
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: system" />
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
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: system" />
                        </FormControl>
                        <FormMessage />
                    </FormItem>
                )}
            />
            <FormField
                control={control}
                name={`${hlsPath}.base.user_agent`}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel className="text-xs">
                            <Trans>User Agent</Trans>
                        </FormLabel>
                        <FormControl>
                            <Input {...field} className="h-8 text-xs font-mono" placeholder="Default: mesio/version" />
                        </FormControl>
                        <FormMessage />
                    </FormItem>
                )}
            />
        </div>
        <div className="flex gap-4">
            <FormField
                control={control}
                name={`${hlsPath}.base.http_version`}
                render={({ field }) => (
                    <FormItem className="flex-1">
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
                                <SelectItem value="auto"><Trans>Auto</Trans></SelectItem>
                                <SelectItem value="http2_only"><Trans>HTTP/2 Only</Trans></SelectItem>
                                <SelectItem value="http1_only"><Trans>HTTP/1.1 Only</Trans></SelectItem>
                            </SelectContent>
                        </Select>
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
                            <Trans>Follow Redirects</Trans>
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
                            <Trans>Accept Invalid Certs</Trans>
                        </FormLabel>
                        <FormControl>
                            <Switch
                                checked={field.value}
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
                name={`${hlsPath}.playlist_config.live_refresh_interval_ms`}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel className="text-xs">
                            <Trans>Live Refresh Interval (ms)</Trans>
                        </FormLabel>
                        <FormControl>
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: 1000" />
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
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: 5" />
                        </FormControl>
                        <FormMessage />
                    </FormItem>
                )}
            />
        </div>
        <FormField
            control={control}
            name={`${hlsPath}.playlist_config.adaptive_refresh_enabled`}
            render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
                    <div className="space-y-0.5">
                        <FormLabel className="text-xs font-normal">
                            <Trans>Adaptive Refresh</Trans>
                        </FormLabel>
                        <FormDescription className="text-[10px]">
                            <Trans>Adjust refresh rate based on target duration</Trans>
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
    </div>
));
HlsPlaylistSettings.displayName = 'HlsPlaylistSettings';

const HlsFetcherSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
        <FormField
            control={control}
            name={`${hlsPath}.download_concurrency`}
            render={({ field }) => (
                <FormItem>
                    <FormLabel className="text-xs">
                        <Trans>Download Concurrency</Trans>
                    </FormLabel>
                    <FormControl>
                        <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: 5" />
                    </FormControl>
                    <FormDescription className="text-[10px]">
                        <Trans>Number of parallel segment downloads</Trans>
                    </FormDescription>
                    <FormMessage />
                </FormItem>
            )}
        />
        <div className="grid gap-4 sm:grid-cols-2">
            <FormField
                control={control}
                name={`${hlsPath}.fetcher_config.segment_download_timeout_ms`}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel className="text-xs">
                            <Trans>Segment Download Timeout (ms)</Trans>
                        </FormLabel>
                        <FormControl>
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: 10000" />
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
                            <Input type="number" {...field} className="h-8 text-xs font-mono" placeholder="Default: 3" />
                        </FormControl>
                        <FormMessage />
                    </FormItem>
                )}
            />
        </div>
    </div>
));
HlsFetcherSettings.displayName = 'HlsFetcherSettings';

const HlsPerformanceSettings = React.memo(({ control, hlsPath }: SubFormProps) => (
    <div className="space-y-4 animate-in fade-in duration-300">
        <div className="grid gap-2">
            <FormField
                control={control}
                name={`${hlsPath}.performance_config.zero_copy_enabled`}
                render={({ field }) => (
                    <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-muted/5 p-3 py-2 shadow-sm">
                        <FormLabel className="text-xs font-normal">
                            <Trans>Zero Copy Processing</Trans>
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
                            <Trans>Offload Decryption</Trans>
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
    </div>
));
HlsPerformanceSettings.displayName = 'HlsPerformanceSettings';

interface MesioHlsFormProps {
    control: Control<any>;
    basePath?: string;
}

export function MesioHlsForm({ control, basePath = 'config' }: MesioHlsFormProps) {
    const hlsPath = `${basePath}.hls`;

    return (
        <Card className="border-border/40 bg-background/20 shadow-none overflow-hidden animate-in fade-in slide-in-from-top-1 duration-200">
            <CardContent className="p-3">
                <Tabs defaultValue="base" className="w-full">
                    <TabsList className="grid w-full grid-cols-4 mb-4 bg-muted/30 p-1 h-10">
                        <TabsTrigger value="base" className="text-[10px] gap-1 px-1">
                            <Globe className="w-3 h-3 text-sky-500" />
                            <span className="hidden sm:inline"><Trans>Base</Trans></span>
                        </TabsTrigger>
                        <TabsTrigger value="playlist" className="text-[10px] gap-1 px-1">
                            <ListMusic className="w-3 h-3 text-pink-500" />
                            <span className="hidden sm:inline"><Trans>Playlist</Trans></span>
                        </TabsTrigger>
                        <TabsTrigger value="fetcher" className="text-[10px] gap-1 px-1">
                            <Bot className="w-3 h-3 text-purple-500" />
                            <span className="hidden sm:inline"><Trans>Fetcher</Trans></span>
                        </TabsTrigger>
                        <TabsTrigger value="performance" className="text-[10px] gap-1 px-1">
                            <Zap className="w-3 h-3 text-yellow-500" />
                            <span className="hidden sm:inline"><Trans>Perf</Trans></span>
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
                </Tabs>
            </CardContent>
        </Card>
    );
}
