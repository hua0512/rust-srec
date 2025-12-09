import { Control } from 'react-hook-form';
import { FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { ListInput } from '@/components/ui/list-input';
import { Card, CardContent } from '@/components/ui/card';
import { Terminal, Settings, Command } from 'lucide-react';
import { t } from "@lingui/core/macro";
import { Trans } from "@lingui/react/macro";

interface StreamlinkFormProps {
    control: Control<any>;
    basePath?: string;
}

export function StreamlinkForm({ control, basePath = "config" }: StreamlinkFormProps) {
    return (
        <div className="space-y-6">
            <div className="grid gap-4 md:grid-cols-2">
                <FormField
                    control={control}
                    name={`${basePath}.binary_path`}
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel className="flex items-center gap-2">
                                <Terminal className="w-4 h-4 text-primary" />
                                <Trans>Binary Path</Trans>
                            </FormLabel>
                            <FormControl>
                                <Input {...field} placeholder={t`streamlink`} />
                            </FormControl>
                            <FormDescription><Trans>Absolute path or 'streamlink' in PATH</Trans></FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
                <FormField
                    control={control}
                    name={`${basePath}.quality`}
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel className="flex items-center gap-2">
                                <Settings className="w-4 h-4 text-primary" />
                                <Trans>Quality</Trans>
                            </FormLabel>
                            <FormControl>
                                <Input {...field} placeholder={t`best`} />
                            </FormControl>
                            <FormDescription><Trans>e.g. 'best', 'worst', '720p', 'audio_only'</Trans></FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>

            <Card className="bg-muted/30 border-dashed">
                <CardContent className="pt-6">
                    <FormField
                        control={control}
                        name={`${basePath}.extra_args`}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel className="flex items-center gap-2">
                                    <Command className="w-4 h-4 text-primary" />
                                    <Trans>Extra Arguments</Trans>
                                </FormLabel>
                                <FormControl>
                                    <ListInput
                                        value={field.value}
                                        onChange={field.onChange}
                                        placeholder={t`--hls-live-edge 3`}
                                    />
                                </FormControl>
                                <FormDescription><Trans>Any additional command line arguments to pass to Streamlink</Trans></FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </CardContent>
            </Card>
        </div>
    );
}
