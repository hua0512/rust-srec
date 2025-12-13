import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '../../../ui/form';
import { Textarea } from '../../../ui/textarea';
import { Trans } from '@lingui/react/macro';
import { Code } from 'lucide-react';
import { Separator } from '../../../ui/separator';
import { UseFormReturn } from 'react-hook-form';



interface AdvancedTabProps {
    form: UseFormReturn<any>;
    basePath?: string;
}

export function AdvancedTab({ form, basePath }: AdvancedTabProps) {
    return (
        <div className="rounded-xl border bg-card text-card-foreground shadow-sm p-6 space-y-4">
            <div className="flex items-center gap-3 mb-2">
                <div className="p-2 bg-blue-500/10 text-blue-500 rounded-lg">
                    <Code className="w-5 h-5" />
                </div>
                <div>
                    <h3 className="font-semibold"><Trans>Advanced Configuration</Trans></h3>
                    <p className="text-sm text-muted-foreground"><Trans>JSON configurations.</Trans></p>
                </div>
            </div>
            <Separator />

            <FormField
                control={form.control}
                name={basePath ? `${basePath}.stream_selection_config` : "stream_selection_config"}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Stream Selection Config (JSON)</Trans></FormLabel>
                        <FormControl>
                            <Textarea
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                                className="font-mono text-xs min-h-[100px]"
                                placeholder='{"keyword": "...", "quality": "..."}'
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>JSON configuration for selecting streams.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <FormField
                control={form.control}
                name={basePath ? `${basePath}.platform_specific_config` : "platform_specific_config"}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Legacy Platform Specific Config (JSON)</Trans></FormLabel>
                        <FormControl>
                            <Textarea
                                placeholder='{"output_folder": "./custom"}'
                                className="font-mono text-xs min-h-[200px]"
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Legacy JSON blob. Use explicit fields in General tab preferred.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <FormField
                control={form.control}
                name={basePath ? `${basePath}.download_retry_policy` : "download_retry_policy"}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Retry Policy (JSON)</Trans></FormLabel>
                        <FormControl>
                            <Textarea
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                                className="font-mono text-xs min-h-[100px]"
                                placeholder='{"max_retries": 10, "retry_delay": 10}'
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Advanced retry policy.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <FormField
                control={form.control}
                name={basePath ? `${basePath}.event_hooks` : "event_hooks"}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Event Hooks (JSON)</Trans></FormLabel>
                        <FormControl>
                            <Textarea
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                                className="font-mono text-xs min-h-[100px]"
                                placeholder='{"on_start": "cmd"}'
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>External commands hook.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />
            <FormField
                control={form.control}
                name={basePath ? `${basePath}.pipeline` : "pipeline"}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Pipeline Configuration (JSON)</Trans></FormLabel>
                        <FormControl>
                            <Textarea
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                                className="font-mono text-xs min-h-[100px]"
                                placeholder='[{"type":"remux",...}]'
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Platform-specific pipeline steps.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />
        </div>
    );
}
