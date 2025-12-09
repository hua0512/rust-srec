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
import { InputWithUnit } from "@/components/ui/input-with-unit";
import { HardDrive } from "lucide-react";
import { Trans } from "@lingui/react/macro";

interface ResourceLimitsCardProps {
    control: Control<any>;
}

export function ResourceLimitsCard({ control }: ResourceLimitsCardProps) {
    return (
        <Card className="hover:shadow-md transition-all duration-300 border-muted/60">
            <CardHeader>
                <CardTitle className="flex items-center gap-3 text-xl">
                    <div className="p-2.5 bg-orange-500/10 text-orange-500 rounded-lg">
                        <HardDrive className="w-5 h-5" />
                    </div>
                    <Trans>Resource Limits</Trans>
                </CardTitle>
                <CardDescription className="pl-[3.25rem]">
                    <Trans>Size and duration constraints for recordings.</Trans>
                </CardDescription>
            </CardHeader>
            <CardContent className="space-y-6">
                <FormField
                    control={control}
                    name="min_segment_size_bytes"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel>
                                <Trans>Min Segment Size</Trans>
                            </FormLabel>
                            <FormControl>
                                <InputWithUnit
                                    unitType="size"
                                    value={field.value}
                                    onChange={field.onChange}
                                    placeholder="0"
                                />
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
                <FormField
                    control={control}
                    name="max_download_duration_secs"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel>
                                <Trans>Max Duration</Trans>
                            </FormLabel>
                            <FormControl>
                                <InputWithUnit
                                    unitType="duration"
                                    value={field.value}
                                    onChange={field.onChange}
                                    placeholder="Unlimited"
                                />
                            </FormControl>
                            <FormDescription className="text-xs">
                                <Trans>0 = Unlimited</Trans>
                            </FormDescription>
                            <FormMessage />
                        </FormItem>
                    )}
                />
                <FormField
                    control={control}
                    name="max_part_size_bytes"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel>
                                <Trans>Max Part Size</Trans>
                            </FormLabel>
                            <FormControl>
                                <InputWithUnit
                                    unitType="size"
                                    value={field.value}
                                    onChange={field.onChange}
                                    placeholder="0"
                                />
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </CardContent>
        </Card>
    );
}
