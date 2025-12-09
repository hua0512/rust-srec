import { Control } from 'react-hook-form';
import { FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { ListInput } from '@/components/ui/list-input';
import { Card, CardContent } from '@/components/ui/card';
import { Separator } from '@/components/ui/separator';
import { Terminal, Clock, Shield } from 'lucide-react';
import { t } from "@lingui/core/macro";
import { Trans } from "@lingui/react/macro";

interface FfmpegFormProps {
    control: Control<any>;
    basePath?: string;
}

export function FfmpegForm({ control, basePath = "config" }: FfmpegFormProps) {
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
                                <Input {...field} placeholder={t`/usr/bin/ffmpeg or ffmpeg`} />
                            </FormControl>
                            <FormDescription><Trans>Absolute path or 'ffmpeg' if in PATH</Trans></FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
                <FormField
                    control={control}
                    name={`${basePath}.timeout_secs`}
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel className="flex items-center gap-2">
                                <Clock className="w-4 h-4 text-primary" />
                                <Trans>Timeout</Trans>
                            </FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <Input type="number" {...field} className="pr-12" />
                                    <span className="absolute right-3 top-2.5 text-xs text-muted-foreground"><Trans>secs</Trans></span>
                                </div>
                            </FormControl>
                            <FormDescription><Trans>Connection/activity timeout</Trans></FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>

            <FormField
                control={control}
                name={`${basePath}.user_agent`}
                render={({ field }) => (
                    <FormItem>
                        <FormLabel className="flex items-center gap-2">
                            <Shield className="w-4 h-4 text-primary" />
                            <Trans>User Agent</Trans>
                        </FormLabel>
                        <FormControl>
                            <Input {...field} placeholder={t`Mozilla/5.0...`} />
                        </FormControl>
                        <FormDescription><Trans>Custom User-Agent string (Optional)</Trans></FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <Separator />

            <div className="grid gap-6 md:grid-cols-2">
                <Card className="bg-muted/30 border-dashed">
                    <CardContent className="pt-6">
                        <FormField
                            control={control}
                            name={`${basePath}.input_args`}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel><Trans>Input Arguments</Trans></FormLabel>
                                    <FormControl>
                                        <ListInput
                                            value={field.value}
                                            onChange={field.onChange}
                                            placeholder={t`-reconnect 1`}
                                        />
                                    </FormControl>
                                    <FormDescription><Trans>Args inserted before -i input_url</Trans></FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </CardContent>
                </Card>

                <Card className="bg-muted/30 border-dashed">
                    <CardContent className="pt-6">
                        <FormField
                            control={control}
                            name={`${basePath}.output_args`}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel><Trans>Output Arguments</Trans></FormLabel>
                                    <FormControl>
                                        <ListInput
                                            value={field.value}
                                            onChange={field.onChange}
                                            placeholder={t`-c copy`}
                                        />
                                    </FormControl>
                                    <FormDescription><Trans>Args used for processing/encoding</Trans></FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </CardContent>
                </Card>
            </div>
        </div>
    );
}
