import { useFormContext } from 'react-hook-form';
import { FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/macro';
import { cn } from '@/lib/utils';

const DAYS = [
    { id: 'Mon', label: 'M' },
    { id: 'Tue', label: 'T' },
    { id: 'Wed', label: 'W' },
    { id: 'Thu', label: 'T' },
    { id: 'Fri', label: 'F' },
    { id: 'Sat', label: 'S' },
    { id: 'Sun', label: 'S' },
];

export function TimeBasedFilterForm() {
    const { control } = useFormContext();

    return (
        <div className="space-y-6 p-4">
            <FormField
                control={control}
                name="config.days"
                render={({ field }) => (
                    <FormItem className="space-y-3">
                        <div>
                            <FormLabel className="text-base font-semibold"><Trans>Active Days</Trans></FormLabel>
                            <FormDescription>
                                <Trans>Select the days required for this filter to apply.</Trans>
                            </FormDescription>
                        </div>
                        <div className="flex gap-2 justify-between">
                            {DAYS.map((day) => {
                                const isSelected = field.value?.includes(day.id);
                                return (
                                    <div
                                        key={day.id}
                                        className={cn(
                                            "h-10 w-10 rounded-full flex items-center justify-center cursor-pointer transition-all border font-medium text-sm",
                                            isSelected
                                                ? "bg-primary text-primary-foreground border-primary shadow-md scale-110"
                                                : "bg-background hover:bg-muted text-muted-foreground border-muted"
                                        )}
                                        onClick={() => {
                                            const current = field.value || [];
                                            const updated = current.includes(day.id)
                                                ? current.filter((d: string) => d !== day.id)
                                                : [...current, day.id];
                                            field.onChange(updated);
                                        }}
                                    >
                                        {day.label}
                                    </div>
                                );
                            })}
                        </div>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <div className="grid grid-cols-2 gap-6">
                <FormField
                    control={control}
                    name="config.start_time"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel className="font-semibold"><Trans>Start Time</Trans></FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <Input type="time" step="1" {...field} className="font-mono" />
                                </div>
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />

                <FormField
                    control={control}
                    name="config.end_time"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel className="font-semibold"><Trans>End Time</Trans></FormLabel>
                            <FormControl>
                                <Input type="time" step="1" {...field} className="font-mono" />
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>
            <div className="rounded-lg bg-blue-500/10 text-blue-700 dark:text-blue-300 p-3 text-xs text-center border border-blue-200 dark:border-blue-800">
                <Trans>Filter applies if current time is between Start and End time.</Trans>
            </div>
        </div>
    );
}
