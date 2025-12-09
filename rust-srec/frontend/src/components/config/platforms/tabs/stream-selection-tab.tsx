
// Cleaned up unused imports
import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { Filter, Zap } from 'lucide-react';
import { FormControl, FormDescription, FormItem, FormLabel } from '../../../ui/form';
import { Separator } from '../../../ui/separator';
import { Input } from '../../../ui/input';
import { TagInput } from '../../../ui/tag-input';
import { StreamSelectionConfigObjectSchema } from '../../../../api/schemas';

type StreamSelectionConfig = z.infer<typeof StreamSelectionConfigObjectSchema>;

interface StreamSelectionTabProps {
    form: UseFormReturn<any>;
    basePath?: string;
}

export function StreamSelectionTab({ form, basePath }: StreamSelectionTabProps) {
    const fieldName = basePath ? `${basePath}.stream_selection_config` : "stream_selection_config";
    const rawConfig = form.watch(fieldName);

    // Helper to parse JSON string to object
    const parseConfig = (jsonString: string | null | undefined): StreamSelectionConfig => {
        if (!jsonString) return {};
        try {
            return JSON.parse(jsonString);
        } catch (e) {
            console.error("Failed to parse stream selection config:", e);
            return {};
        }
    };

    const currentConfig = parseConfig(rawConfig);

    // Generic handler to update a specific field in the JSON string
    const updateField = <K extends keyof StreamSelectionConfig>(
        field: K,
        value: StreamSelectionConfig[K]
    ) => {
        const newConfig = { ...currentConfig, [field]: value };
        // Remove empty arrays/undefined to keep JSON clean
        if (Array.isArray(value) && value.length === 0) {
            delete newConfig[field];
        }
        if (value === undefined || value === null) {
            delete newConfig[field];
        }

        // If object is empty, set to null
        if (Object.keys(newConfig).length === 0) {
            form.setValue(fieldName, null, { shouldDirty: true });
        } else {
            form.setValue(fieldName, JSON.stringify(newConfig), { shouldDirty: true });
        }
    };

    return (
        <div className="grid gap-6">
            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Filter className="w-4 h-4" /> <Trans>Preferences</Trans>
                </h3>
                <Separator />

                <div className="grid grid-cols-1 gap-4">
                    <FormItem>
                        <FormLabel><Trans>Preferred Qualities</Trans></FormLabel>
                        <FormControl>
                            <TagInput
                                value={currentConfig.preferred_qualities || []}
                                onChange={(tags) => updateField("preferred_qualities", tags)}
                                placeholder="e.g. 1080p, source, 原画"
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Prioritize specific qualities (case-insensitive).</Trans>
                        </FormDescription>
                    </FormItem>

                    <FormItem>
                        <FormLabel><Trans>Preferred Formats</Trans></FormLabel>
                        <FormControl>
                            <TagInput
                                value={currentConfig.preferred_formats || []}
                                onChange={(tags) => updateField("preferred_formats", tags)}
                                placeholder="e.g. flv, hls"
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Prioritize specific streaming protocols.</Trans>
                        </FormDescription>
                    </FormItem>

                    <FormItem>
                        <FormLabel><Trans>Preferred CDNs</Trans></FormLabel>
                        <FormControl>
                            <TagInput
                                value={currentConfig.preferred_cdns || []}
                                onChange={(tags) => updateField("preferred_cdns", tags)}
                                placeholder="e.g. aliyun, akamaized"
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Prioritize specific CDN providers.</Trans>
                        </FormDescription>
                    </FormItem>
                </div>
            </div>

            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground flex items-center gap-2">
                    <Zap className="w-4 h-4" /> <Trans>Limits</Trans>
                </h3>
                <Separator />

                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <FormItem>
                        <FormLabel><Trans>Min Bitrate (bps)</Trans></FormLabel>
                        <FormControl>
                            <Input
                                type="number"
                                value={currentConfig.min_bitrate ?? ''}
                                onChange={(e) => updateField("min_bitrate", e.target.value ? Number(e.target.value) : undefined)}
                                placeholder="No limit"
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Ignore streams below this bitrate.</Trans>
                        </FormDescription>
                    </FormItem>

                    <FormItem>
                        <FormLabel><Trans>Max Bitrate (bps)</Trans></FormLabel>
                        <FormControl>
                            <Input
                                type="number"
                                value={currentConfig.max_bitrate ?? ''}
                                onChange={(e) => updateField("max_bitrate", e.target.value ? Number(e.target.value) : undefined)}
                                placeholder="No limit"
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Ignore streams above this bitrate.</Trans>
                        </FormDescription>
                    </FormItem>
                </div>
            </div>
        </div>
    );
}
