import { UseFormReturn } from 'react-hook-form';
import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '../../ui/form';
import { Input } from '../../ui/input';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '../../ui/select';
import { Checkbox } from '../../ui/checkbox';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Link, User } from 'lucide-react';
import { PlatformConfig, Template } from '../../../api/schemas'; // ensure correct import path

interface StreamerGeneralSettingsProps {
    form: UseFormReturn<any>;
    platformConfigs?: PlatformConfig[];
    templates?: Template[];
    isLoading?: boolean;
}

export function StreamerGeneralSettings({ form, platformConfigs, templates, isLoading }: StreamerGeneralSettingsProps) {
    return (
        <div className="space-y-6">
            <div className="space-y-4">
                <h3 className="text-sm font-medium text-muted-foreground uppercase tracking-wider mb-2">
                    <Trans>Stream Details</Trans>
                </h3>
                <FormField
                    control={form.control}
                    name="url"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>URL</Trans></FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <Link className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                    <Input
                                        placeholder="https://twitch.tv/..."
                                        {...field}
                                        className="bg-background/50 font-mono text-sm pl-9"
                                    />
                                </div>
                            </FormControl>
                            <FormDescription>
                                <Trans>The direct link to the channel or stream.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={form.control}
                    name="name"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Name</Trans></FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <User className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                    <Input
                                        placeholder={t`e.g. My Favorite Streamer`}
                                        {...field}
                                        className="bg-background/50 pl-9"
                                    />
                                </div>
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <FormField
                    control={form.control}
                    name="platform_config_id"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Platform Configuration</Trans></FormLabel>
                            <Select
                                onValueChange={(val) => field.onChange(val === "none" ? null : Number(val))}
                                value={field.value ? String(field.value) : "none"}
                            >
                                <FormControl>
                                    <SelectTrigger className="bg-background/50">
                                        <SelectValue placeholder={t`Select config`} />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="none"><Trans>None (Default)</Trans></SelectItem>
                                    {platformConfigs?.map((platform) => (
                                        <SelectItem key={platform.id} value={String(platform.id)}>
                                            {platform.name}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                            <FormDescription>
                                <Trans>Specific settings for the platform.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={form.control}
                    name="template_id"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Template</Trans></FormLabel>
                            <Select
                                onValueChange={(val) => field.onChange(val === "none" ? null : Number(val))}
                                value={field.value ? String(field.value) : "none"}
                            >
                                <FormControl>
                                    <SelectTrigger className="bg-background/50">
                                        <SelectValue placeholder={t`Select template`} />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="none"><Trans>None (Default)</Trans></SelectItem>
                                    {templates?.map((template) => (
                                        <SelectItem key={template.id} value={String(template.id)}>
                                            {template.name}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                            <FormDescription>
                                <Trans>Apply template settings.</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>

            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <FormField
                    control={form.control}
                    name="priority"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Priority</Trans></FormLabel>
                            <Select onValueChange={field.onChange} value={field.value}>
                                <FormControl>
                                    <SelectTrigger className="bg-background/50">
                                        <SelectValue placeholder={t`Select priority`} />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="HIGH"><Trans>High</Trans></SelectItem>
                                    <SelectItem value="NORMAL"><Trans>Normal</Trans></SelectItem>
                                    <SelectItem value="LOW"><Trans>Low</Trans></SelectItem>
                                </SelectContent>
                            </Select>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={form.control}
                    name="enabled"
                    render={({ field }) => (
                        <FormItem className="flex flex-row items-start space-x-3 space-y-0 rounded-lg border border-border/50 p-4 bg-muted/20 mt-1">
                            <FormControl>
                                <Checkbox
                                    checked={field.value}
                                    onCheckedChange={field.onChange}
                                />
                            </FormControl>
                            <div className="space-y-1 leading-none">
                                <FormLabel className="font-semibold cursor-pointer">
                                    <Trans>Enable Monitoring</Trans>
                                </FormLabel>
                                <FormDescription>
                                    <Trans>Start checking this streamer immediately.</Trans>
                                </FormDescription>
                            </div>
                        </FormItem>
                    )}
                />
            </div>
        </div>
    );
}
