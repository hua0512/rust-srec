import { useFormContext } from 'react-hook-form';
import { FormControl, FormDescription, FormField, FormItem, FormLabel, FormMessage } from '../../ui/form';
import { Textarea } from '../../ui/textarea';
import { Switch } from '../../ui/switch';
import { Trans } from '@lingui/macro';

export function RegexFilterForm() {
    const { control } = useFormContext();

    return (
        <div className="space-y-4">
            <FormField
                control={control}
                name="config.pattern"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Regex Pattern</Trans></FormLabel>
                        <FormControl>
                            <Textarea {...field} placeholder="^Started.*" className="font-mono" />
                        </FormControl>
                        <FormDescription>
                            <Trans>Rust regex syntax supported.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <FormField
                control={control}
                name="config.exclude"
                render={({ field }) => (
                    <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
                        <div className="space-y-0.5">
                            <FormLabel className="text-base"><Trans>Exclude</Trans></FormLabel>
                            <FormDescription>
                                <Trans>If enabled, streams matching this pattern will be ignored.</Trans>
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
                name="config.case_insensitive"
                render={({ field }) => (
                    <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
                        <div className="space-y-0.5">
                            <FormLabel className="text-base"><Trans>Case Insensitive</Trans></FormLabel>
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
    );
}
