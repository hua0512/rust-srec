import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '../../../ui/form';

import { Input } from '../../../ui/input';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Clock, Download, Shield, Tv } from 'lucide-react';
import { Separator } from '../../../ui/separator';
import { InputWithUnit } from '../../../ui/input-with-unit';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../../../ui/select';
import { UseFormReturn } from 'react-hook-form';

interface GeneralTabProps {
    form: UseFormReturn<any>;
    basePath?: string;
}

export function GeneralTab({ form, basePath }: GeneralTabProps) {
    return (
        <div className="grid gap-6">
            <FormField
                control={form.control}
                name={basePath ? `${basePath}.record_danmu` : "record_danmu"}
                render={({ field }) => (
                    <FormItem className="flex flex-row items-center justify-between rounded-xl border p-4 shadow-sm bg-card hover:bg-accent/5 transition-colors">
                        <div className="space-y-0.5">
                            <FormLabel className="text-base font-semibold flex items-center gap-2">
                                <Tv className="w-4 h-4 text-primary" />
                                <Trans>Record Danmu</Trans>
                            </FormLabel>
                            <FormDescription>
                                <Trans>Capture real-time comments and chat messages if available.</Trans>
                            </FormDescription>
                        </div>
                        <FormControl>
                            <Select
                                value={field.value === null || field.value === undefined ? "null" : field.value ? "true" : "false"}
                                onValueChange={(v) => {
                                    if (v === "null") field.onChange(null);
                                    else if (v === "true") field.onChange(true);
                                    else field.onChange(false);
                                }}
                            >
                                <FormControl>
                                    <SelectTrigger>
                                        <SelectValue placeholder="Select behavior" />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="null">Global Default</SelectItem>
                                    <SelectItem value="true">Enabled</SelectItem>
                                    <SelectItem value="false">Disabled</SelectItem>
                                </SelectContent>
                            </Select>
                        </FormControl>
                    </FormItem>
                )}
            />

            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Clock className="w-4 h-4" /> <Trans>Timing & Delays</Trans>
                </h3>
                <Separator />
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.fetch_delay_ms` : "fetch_delay_ms"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Fetch Delay</Trans></FormLabel>
                                <FormControl>
                                    <InputWithUnit
                                        value={field.value !== null && field.value !== undefined ? field.value / 1000 : null}
                                        onChange={(v) => field.onChange(v !== null ? Math.round(v * 1000) : null)}
                                        unitType="duration"
                                        placeholder={t`Global Default`}
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Delay between checks.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.download_delay_ms` : "download_delay_ms"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Download Delay</Trans></FormLabel>
                                <FormControl>
                                    <InputWithUnit
                                        value={field.value !== null && field.value !== undefined ? field.value / 1000 : null}
                                        onChange={(v) => field.onChange(v !== null ? Math.round(v * 1000) : null)}
                                        unitType="duration"
                                        placeholder={t`Global Default`}
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Delay before download.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </div>
            </div>

            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Download className="w-4 h-4" /> <Trans>Output Settings</Trans>
                </h3>
                <Separator />

                <FormField
                    control={form.control}
                    name={basePath ? `${basePath}.output_folder` : "output_folder"}
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Output Folder</Trans></FormLabel>
                            <FormControl>
                                <Input
                                    {...field}
                                    value={field.value ?? ''}
                                    onChange={(e) => field.onChange(e.target.value || null)}
                                    placeholder="./downloads"
                                />
                            </FormControl>
                            <FormDescription>
                                <Trans>Override output folder for this platform.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={form.control}
                    name={basePath ? `${basePath}.output_filename_template` : "output_filename_template"}
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Filename Template</Trans></FormLabel>
                            <FormControl>
                                <Input
                                    {...field}
                                    value={field.value ?? ''}
                                    onChange={(e) => field.onChange(e.target.value || null)}
                                    placeholder="{streamer}-{title}-%Y%m%d-%H%M%S"
                                />
                            </FormControl>
                            <FormDescription>
                                <Trans>Override filename template.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={form.control}
                    name={basePath ? `${basePath}.output_file_format` : "output_file_format"}
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Output Format</Trans></FormLabel>
                            <Select
                                onValueChange={(val) => field.onChange(val === "default" ? null : val)}
                                defaultValue={field.value || "default"}
                            >
                                <FormControl>
                                    <SelectTrigger>
                                        <SelectValue placeholder="Select a format" />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="default"><Trans>Global Default</Trans></SelectItem>
                                    <SelectItem value="mp4">MP4</SelectItem>
                                    <SelectItem value="flv">FLV</SelectItem>
                                    <SelectItem value="mkv">MKV</SelectItem>
                                    <SelectItem value="ts">TS</SelectItem>
                                </SelectContent>
                            </Select>
                            <FormDescription>
                                <Trans>Force specific output format.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.download_engine` : "download_engine"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Download Engine</Trans></FormLabel>
                                <FormControl>
                                    <Input
                                        {...field}
                                        value={field.value ?? ''}
                                        onChange={(e) => field.onChange(e.target.value || null)}
                                        placeholder="mesio"
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Override download engine.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />

                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.max_bitrate` : "max_bitrate"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Max Bitrate (Kbps)</Trans></FormLabel>
                                <FormControl>
                                    <Input
                                        type="number"
                                        {...field}
                                        value={field.value ?? ''}
                                        onChange={(e) => field.onChange(e.target.value ? Number(e.target.value) : null)}
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Max bitrate limit.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </div>
            </div>

            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Shield className="w-4 h-4" /> <Trans>Limits & Validation</Trans>
                </h3>
                <Separator />
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.max_download_duration_secs` : "max_download_duration_secs"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Max Duration</Trans></FormLabel>
                                <FormControl>
                                    <InputWithUnit
                                        value={field.value ?? null}
                                        onChange={field.onChange}
                                        unitType="duration"
                                        placeholder={t`Global Default`}
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Split after duration.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.min_segment_size_bytes` : "min_segment_size_bytes"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Min Segment Size</Trans></FormLabel>
                                <FormControl>
                                    <InputWithUnit
                                        value={field.value ?? null}
                                        onChange={field.onChange}
                                        unitType="size"
                                        placeholder={t`Global Default`}
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Min size to keep.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                    <FormField
                        control={form.control}
                        name={basePath ? `${basePath}.max_part_size_bytes` : "max_part_size_bytes"}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Max Part Size</Trans></FormLabel>
                                <FormControl>
                                    <InputWithUnit
                                        value={field.value ?? null}
                                        onChange={field.onChange}
                                        unitType="size"
                                        placeholder={t`Global Default`}
                                    />
                                </FormControl>
                                <FormDescription>
                                    <Trans>Split after size.</Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </div>
            </div>
        </div>
    );
}
