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
import { t } from '@lingui/core/macro';
import { Shield, Code, Braces } from 'lucide-react';
import { Separator } from '../../../ui/separator';
import { InputWithUnit } from '../../../ui/input-with-unit';
import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { UpdateTemplateRequestSchema } from '../../../../api/schemas';

type EditTemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;

interface AdvancedTabProps {
    form: UseFormReturn<EditTemplateFormValues>;
}

export function AdvancedTab({ form }: AdvancedTabProps) {
    return (
        <div className="grid gap-6">
            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Shield className="w-4 h-4" /> <Trans>Limits & Validation</Trans>
                </h3>
                <Separator />
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <FormField
                        control={form.control}
                        name="max_download_duration_secs"
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
                        name="min_segment_size_bytes"
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
                        name="max_part_size_bytes"
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

            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Code className="w-4 h-4" /> <Trans>Raw Configuration</Trans>
                </h3>
                <Separator />





                <FormField
                    control={form.control}
                    name="event_hooks"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel className="flex items-center gap-2 font-mono text-xs uppercase tracking-wider">
                                <Braces className="w-3 h-3" />
                                <Trans>Event Hooks (JSON)</Trans>
                            </FormLabel>
                            <FormControl>
                                <Textarea
                                    {...field}
                                    value={field.value ?? ''}
                                    onChange={(e) => field.onChange(e.target.value || null)}
                                    placeholder="{}"
                                    className="font-mono text-xs min-h-[100px]"
                                />
                            </FormControl>
                            <FormDescription>
                                <Trans>Define webhooks for events.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>
        </div>
    );
}
