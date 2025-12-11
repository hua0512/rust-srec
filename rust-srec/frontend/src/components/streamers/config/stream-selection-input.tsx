import {
    FormControl,
    FormDescription,
    FormItem,
    FormLabel,
} from '../../ui/form';
import { Input } from '../../ui/input';
import { TagInput } from '../../ui/tag-input';
import { Trans } from '@lingui/react/macro';
import { Filter, Zap } from 'lucide-react';
import { Separator } from '../../ui/separator';

export interface StreamSelectionConfig {
    preferred_qualities?: string[];
    preferred_formats?: string[];
    preferred_cdns?: string[];
    min_bitrate?: number;
    max_bitrate?: number;
}

interface StreamSelectionInputProps {
    value?: StreamSelectionConfig;
    onChange: (value: StreamSelectionConfig) => void;
}

export function StreamSelectionInput({ value = {}, onChange }: StreamSelectionInputProps) {
    const updateField = <K extends keyof StreamSelectionConfig>(
        field: K,
        fieldValue: StreamSelectionConfig[K]
    ) => {
        const newConfig = { ...value, [field]: fieldValue };

        // Cleanup empty arrays/undefined
        if (Array.isArray(fieldValue) && fieldValue.length === 0) {
            delete newConfig[field];
        }
        if (fieldValue === undefined || fieldValue === null) {
            // @ts-ignore
            delete newConfig[field];
        }
        // Bitrates that are NaN should be removed? 
        // Logic handled by parent regarding cleaning empty objects, 
        // but here we ensure the *fields* are clean.

        onChange(newConfig);
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
                                value={value.preferred_qualities || []}
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
                                value={value.preferred_formats || []}
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
                                value={value.preferred_cdns || []}
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

                <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                    <FormItem>
                        <FormLabel><Trans>Min Bitrate (bps)</Trans></FormLabel>
                        <FormControl>
                            <Input
                                type="number"
                                min={0}
                                value={value.min_bitrate ?? ''}
                                onChange={(e) => updateField("min_bitrate", e.target.value ? Number(e.target.value) : undefined)}
                                placeholder="No limit"
                                className="bg-background/80"
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
                                min={0}
                                value={value.max_bitrate ?? ''}
                                onChange={(e) => updateField("max_bitrate", e.target.value ? Number(e.target.value) : undefined)}
                                placeholder="No limit"
                                className="bg-background/80"
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
