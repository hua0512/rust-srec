import { Control } from "react-hook-form";
import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
} from "@/components/ui/card";
import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Cpu } from "lucide-react";
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";
import { EngineConfigSchema } from "@/api/schemas";
import { z } from "zod";

interface ConcurrencyCardProps {
    control: Control<any>;
    engines?: z.infer<typeof EngineConfigSchema>[];
    enginesLoading?: boolean;
}

export function ConcurrencyCard({
    control,
    engines,
    enginesLoading,
}: ConcurrencyCardProps) {
    return (
        <Card className="hover:shadow-md transition-all duration-300 border-muted/60">
            <CardHeader>
                <CardTitle className="flex items-center gap-3 text-xl">
                    <div className="p-2.5 bg-green-500/10 text-green-500 rounded-lg">
                        <Cpu className="w-5 h-5" />
                    </div>
                    <Trans>Concurrency & Performance</Trans>
                </CardTitle>
                <CardDescription className="pl-[3.25rem]">
                    <Trans>Job limits and engine settings.</Trans>
                </CardDescription>
            </CardHeader>
            <CardContent className="space-y-6">
                <div className="grid grid-cols-2 gap-6">
                    <FormField
                        control={control}
                        name="max_concurrent_downloads"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Max Downloads</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input
                                        type="number"
                                        {...field}
                                        onChange={(e) => field.onChange(Number(e.target.value))}
                                    />
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                    <FormField
                        control={control}
                        name="max_concurrent_uploads"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Max Uploads</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input
                                        type="number"
                                        {...field}
                                        onChange={(e) => field.onChange(Number(e.target.value))}
                                    />
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                    <FormField
                        control={control}
                        name="max_concurrent_cpu_jobs"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Max CPU Jobs</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input
                                        type="number"
                                        {...field}
                                        onChange={(e) => field.onChange(Number(e.target.value))}
                                    />
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                    <FormField
                        control={control}
                        name="max_concurrent_io_jobs"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Max IO Jobs</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input
                                        type="number"
                                        {...field}
                                        onChange={(e) => field.onChange(Number(e.target.value))}
                                    />
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </div>
                <Separator />
                <FormField
                    control={control}
                    name="default_download_engine"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel>
                                <Trans>Default Engine</Trans>
                            </FormLabel>
                            <Select
                                onValueChange={field.onChange}
                                value={field.value}
                                disabled={enginesLoading}
                            >
                                <FormControl>
                                    <SelectTrigger>
                                        <SelectValue placeholder={t`Select a default engine`} />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    {engines?.map((engine) => (
                                        <SelectItem key={engine.id} value={engine.name}>
                                            {engine.name} ({engine.engine_type})
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                            <FormDescription>
                                <Trans>
                                    Engine used for downloads when not specified by
                                    platform/streamer.
                                </Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </CardContent>
        </Card>
    );
}
