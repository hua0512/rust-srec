import { UseFormReturn } from 'react-hook-form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../ui/tabs';
import { Card, CardContent } from '../../ui/card';
import {
    FormControl,
    FormDescription,
    FormItem,
    FormLabel,
} from '../../ui/form';
import { Input } from '../../ui/input';
import { Trans } from '@lingui/react/macro';
import { useEffect, useState } from 'react';
import { StreamSelectionInput, StreamSelectionConfig } from './stream-selection-input';
import { RetryPolicyForm } from './retry-policy-form';
import { DanmuConfigForm } from './danmu-config-form';
import { Filter, FolderOutput, Network, MessageSquare, Cookie } from 'lucide-react';

interface StreamerSpecificConfig {
    output_folder?: string;
    output_filename_template?: string;
    max_part_size_bytes?: number;
    max_download_duration_secs?: number;
    cookies?: string;
    output_file_format?: string;
    min_segment_size_bytes?: number;
    download_engine?: string;
    record_danmu?: boolean;
    stream_selection?: StreamSelectionConfig;
}

interface StreamerConfigurationProps {
    form: UseFormReturn<any>;
}

export function StreamerConfiguration({ form }: StreamerConfigurationProps) {
    // We need to manage streamer_specific_config state here to distribute it across tabs
    const specificConfigJson = form.watch("streamer_specific_config");
    const [specificConfig, setSpecificConfig] = useState<StreamerSpecificConfig>({});

    useEffect(() => {
        if (specificConfigJson) {
            try {
                const parsed = JSON.parse(specificConfigJson);
                setSpecificConfig(parsed);
            } catch (e) {
                // ignore invalid json while typing, or warn
            }
        } else {
            setSpecificConfig({});
        }
    }, []); // Only on mount? No, if external change happens? 
    // Actually, if we use the form as source of truth, we should listen to it.
    // But we are also the writer.
    // Let's use internal state for the UI, and sync to form on change.

    // Better: parse on render/watch, and update form immediately on change.
    // But parsing on every render is fine for small objects.

    const updateSpecificConfig = (newConfig: StreamerSpecificConfig) => {
        setSpecificConfig(newConfig);
        // Clean and serialize
        const cleanConfig = JSON.parse(JSON.stringify(newConfig));
        if (Object.keys(cleanConfig).length === 0) {
            form.setValue("streamer_specific_config", undefined, { shouldDirty: true, shouldValidate: true });
        } else {
            form.setValue("streamer_specific_config", JSON.stringify(cleanConfig), { shouldDirty: true, shouldValidate: true });
        }

    };

    const updateSpecificField = (key: keyof StreamerSpecificConfig, value: any) => {
        const newConfig = { ...specificConfig };
        if (value === '' || value === undefined) {
            delete newConfig[key];
        } else {
            // @ts-ignore
            newConfig[key] = value;
        }
        updateSpecificConfig(newConfig);
    };

    return (
        <Tabs defaultValue="filters" className="w-full">
            <TabsList className="grid w-full grid-cols-2 sm:grid-cols-4 h-auto">
                <TabsTrigger value="filters" className="flex items-center gap-2">
                    <Filter className="w-4 h-4" /> <span className="hidden sm:inline"><Trans>Filters</Trans></span>
                </TabsTrigger>
                <TabsTrigger value="output" className="flex items-center gap-2">
                    <FolderOutput className="w-4 h-4" /> <span className="hidden sm:inline"><Trans>Output</Trans></span>
                </TabsTrigger>
                <TabsTrigger value="network" className="flex items-center gap-2">
                    <Network className="w-4 h-4" /> <span className="hidden sm:inline"><Trans>Network</Trans></span>
                </TabsTrigger>
                <TabsTrigger value="danmu" className="flex items-center gap-2">
                    <MessageSquare className="w-4 h-4" /> <span className="hidden sm:inline"><Trans>Danmu</Trans></span>
                </TabsTrigger>
            </TabsList>

            <div className="mt-4">
                <TabsContent value="filters">
                    <Card className="border-dashed shadow-none">
                        <CardContent className="pt-6">
                            <StreamSelectionInput
                                value={specificConfig.stream_selection}
                                onChange={(val) => updateSpecificField('stream_selection', val)}
                            />
                        </CardContent>
                    </Card>
                </TabsContent>

                <TabsContent value="output">
                    <Card className="border-dashed shadow-none">
                        <CardContent className="pt-6 space-y-4">
                            <FormItem>
                                <FormLabel><Trans>Output Folder</Trans></FormLabel>
                                <FormControl>
                                    <Input
                                        placeholder="/path/to/downloads"
                                        value={specificConfig.output_folder || ''}
                                        onChange={(e) => updateSpecificField('output_folder', e.target.value)}
                                        className="font-mono text-sm"
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Override the default download directory.</Trans>
                                </FormDescription>
                            </FormItem>

                            <FormItem>
                                <FormLabel><Trans>Filename Template</Trans></FormLabel>
                                <FormControl>
                                    <Input
                                        placeholder="{streamer} - {title} - {time}"
                                        value={specificConfig.output_filename_template || ''}
                                        onChange={(e) => updateSpecificField('output_filename_template', e.target.value)}
                                        className="font-mono text-sm"
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Custom filename pattern.</Trans>
                                </FormDescription>
                            </FormItem>

                            <FormItem>
                                <FormLabel><Trans>Output Format</Trans></FormLabel>
                                <FormControl>
                                    <Input
                                        placeholder="flv, mp4, ts, mkv"
                                        value={specificConfig.output_file_format || ''}
                                        onChange={(e) => updateSpecificField('output_file_format', e.target.value)}
                                        className="font-mono text-sm"
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>File format extension.</Trans>
                                </FormDescription>
                            </FormItem>

                            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                                <FormItem>
                                    <FormLabel><Trans>Download Engine</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            placeholder="mesio, ffmpeg, streamlink"
                                            value={specificConfig.download_engine || ''}
                                            onChange={(e) => updateSpecificField('download_engine', e.target.value)}
                                            className="font-mono text-sm"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        <Trans>Engine backend.</Trans>
                                    </FormDescription>
                                </FormItem>
                                <FormItem>
                                    <FormLabel><Trans>Min Segment Size (Bytes)</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            type="number"
                                            min={0}
                                            placeholder="e.g. 1048576"
                                            value={specificConfig.min_segment_size_bytes ?? ''}
                                            onChange={(e) => updateSpecificField('min_segment_size_bytes', e.target.value ? parseInt(e.target.value) : undefined)}
                                            className="font-mono text-sm"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        <Trans>Discard small segments.</Trans>
                                    </FormDescription>
                                </FormItem>
                            </div>

                            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                                <FormItem>
                                    <FormLabel><Trans>Max Part Size (Bytes)</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            type="number"
                                            min={0}
                                            placeholder="e.g. 1073741824"
                                            value={specificConfig.max_part_size_bytes ?? ''}
                                            onChange={(e) => updateSpecificField('max_part_size_bytes', e.target.value ? parseInt(e.target.value) : undefined)}
                                            className="font-mono text-sm"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        <Trans>Split file when size exceeded.</Trans>
                                    </FormDescription>
                                </FormItem>

                                <FormItem>
                                    <FormLabel><Trans>Max Duration (s)</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            type="number"
                                            min={0}
                                            placeholder="e.g. 3600"
                                            value={specificConfig.max_download_duration_secs ?? ''}
                                            onChange={(e) => updateSpecificField('max_download_duration_secs', e.target.value ? parseInt(e.target.value) : undefined)}
                                            className="font-mono text-sm"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        <Trans>Split file when duration exceeded.</Trans>
                                    </FormDescription>
                                </FormItem>
                            </div>
                        </CardContent>
                    </Card>
                </TabsContent>

                <TabsContent value="network">
                    <Card className="border-dashed shadow-none">
                        <CardContent className="pt-6 space-y-6">
                            <FormItem>
                                <FormLabel className="flex items-center gap-2">
                                    <Cookie className="w-4 h-4" /> <Trans>Cookies</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input
                                        placeholder="key=value; key2=value2"
                                        value={specificConfig.cookies || ''}
                                        onChange={(e) => updateSpecificField('cookies', e.target.value)}
                                        className="font-mono text-xs"
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>HTTP cookies for authentication (if required).</Trans>
                                </FormDescription>
                            </FormItem>

                            <div className="space-y-4">
                                <h4 className="text-sm font-medium"><Trans>Download Retry Policy</Trans></h4>
                                <RetryPolicyForm form={form} name="download_retry_policy" />
                            </div>
                        </CardContent>
                    </Card>
                </TabsContent>

                <TabsContent value="danmu">
                    <Card className="border-dashed shadow-none">
                        <CardContent className="pt-6">
                            <div className="border rounded-md p-4">
                                <h4 className="mb-4 text-sm font-medium"><Trans>Danmu Configuration Override</Trans></h4>
                                <div className="space-y-4">
                                    <FormItem>
                                        <FormLabel><Trans>Recording Status</Trans></FormLabel>
                                        <FormControl>
                                            <div className="relative">
                                                <select
                                                    className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
                                                    value={specificConfig.record_danmu === undefined ? 'inherit' : specificConfig.record_danmu.toString()}
                                                    onChange={(e) => {
                                                        const val = e.target.value;
                                                        if (val === 'inherit') {
                                                            updateSpecificField('record_danmu', undefined);
                                                        } else {
                                                            updateSpecificField('record_danmu', val === 'true');
                                                        }
                                                    }}
                                                >
                                                    <option value="inherit">Inherit</option>
                                                    <option value="true">Enabled</option>
                                                    <option value="false">Disabled</option>
                                                </select>
                                            </div>
                                        </FormControl>
                                        <FormDescription>
                                            <Trans>Enable or disable danmu recording explicitly.</Trans>
                                        </FormDescription>
                                    </FormItem>
                                </div>
                            </div>

                            <DanmuConfigForm form={form} name="danmu_sampling_config" />
                        </CardContent>
                    </Card>
                </TabsContent>
            </div>
        </Tabs>
    );
}
