import { Control } from 'react-hook-form';
import { FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from '@/components/ui/card';
import { Database, Wrench, Film } from 'lucide-react';
import { Trans } from "@lingui/react/macro";

interface MesioFormProps {
    control: Control<any>;
    basePath?: string;
}

export function MesioForm({ control, basePath = "config" }: MesioFormProps) {
    return (
        <div className="space-y-6">
            <Card className="bg-primary/5 border-primary/20">
                <CardHeader className="pb-4">
                    <CardTitle className="text-base flex items-center gap-2">
                        <Database className="w-4 h-4" />
                        <Trans>Buffer Settings</Trans>
                    </CardTitle>
                    <CardDescription><Trans>Configure the internal download buffer.</Trans></CardDescription>
                </CardHeader>
                <CardContent>
                    <FormField
                        control={control}
                        name={`${basePath}.buffer_size`}
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>Buffer Size</Trans></FormLabel>
                                <FormControl>
                                    <div className="flex items-center gap-2">
                                        <Input type="number" {...field} />
                                        <span className="text-sm text-muted-foreground whitespace-nowrap"><Trans>bytes</Trans></span>
                                    </div>
                                </FormControl>
                                <FormDescription><Trans>Default: 8388608 (8 MiB)</Trans></FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </CardContent>
            </Card>

            <div className="grid gap-4 md:grid-cols-2">
                <FormField
                    control={control}
                    name={`${basePath}.fix_flv`}
                    render={({ field }) => (
                        <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4 shadow-sm hover:bg-muted/50 transition-colors">
                            <div className="space-y-0.5">
                                <FormLabel className="text-base flex items-center gap-2">
                                    <Film className="w-4 h-4 text-orange-500" />
                                    <Trans>Fix FLV</Trans>
                                </FormLabel>
                                <FormDescription>
                                    <Trans>Attempt to repair timestamps in FLV streams</Trans>
                                </FormDescription>
                            </div>
                            <FormControl>
                                <Switch
                                    checked={field.value}
                                    onCheckedChange={field.onChange}
                                />
                            </FormControl>
                        </FormItem>
                    )}
                />
                <FormField
                    control={control}
                    name={`${basePath}.fix_hls`}
                    render={({ field }) => (
                        <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4 shadow-sm hover:bg-muted/50 transition-colors">
                            <div className="space-y-0.5">
                                <FormLabel className="text-base flex items-center gap-2">
                                    <Wrench className="w-4 h-4 text-blue-500" />
                                    <Trans>Fix HLS</Trans>
                                </FormLabel>
                                <FormDescription>
                                    <Trans>Handle discontinuities in HLS streams</Trans>
                                </FormDescription>
                            </div>
                            <FormControl>
                                <Switch
                                    checked={field.value}
                                    onCheckedChange={field.onChange}
                                />
                            </FormControl>
                        </FormItem>
                    )}
                />
            </div>
        </div>
    );
}
