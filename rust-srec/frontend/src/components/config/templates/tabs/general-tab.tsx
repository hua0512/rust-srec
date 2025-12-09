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
import { Download, Tv, Type } from 'lucide-react';
import { Separator } from '../../../ui/separator';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../../../ui/select';
import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { UpdateTemplateRequestSchema } from '../../../../api/schemas';

type EditTemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;

interface GeneralTabProps {
    form: UseFormReturn<EditTemplateFormValues>;
}

export function GeneralTab({ form }: GeneralTabProps) {
    return (
        <div className="grid gap-6">
            <FormField
                control={form.control}
                name="name"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel className="text-base font-semibold flex items-center gap-2">
                            <Type className="w-4 h-4 text-primary" />
                            <Trans>Template Name</Trans>
                        </FormLabel>
                        <FormControl>
                            <Input {...field} value={field.value ?? ''} placeholder="My Template" />
                        </FormControl>
                        <FormDescription>
                            <Trans>A unique name for this configuration template.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <Separator />

            <FormField
                control={form.control}
                name="record_danmu"
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
                                    <SelectTrigger className="w-[180px]">
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
                    <Download className="w-4 h-4" /> <Trans>Output Settings</Trans>
                </h3>
                <Separator />

                <FormField
                    control={form.control}
                    name="output_folder"
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
                                <Trans>Override output folder.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={form.control}
                    name="output_filename_template"
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
                    name="output_file_format"
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
                        name="download_engine"
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
                </div>
            </div>
        </div>
    );
}
