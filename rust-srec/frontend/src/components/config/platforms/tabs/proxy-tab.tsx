import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '../../../ui/form';
import { UseFormReturn } from 'react-hook-form';
import { ProxyConfigSettings } from '../../proxy-config-settings';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../../../ui/select';
import { Trans } from '@lingui/react/macro';


interface ProxyTabProps {
    form: UseFormReturn<any>;
    basePath?: string;
}

export function ProxyTab({ form, basePath }: ProxyTabProps) {
    return (
        <FormField
            control={form.control}
            name={basePath ? `${basePath}.proxy_config` : "proxy_config"}
            render={({ field }) => {
                const isInherited = field.value === null || field.value === undefined;

                return (
                    <FormItem className="space-y-4">
                        <div className="flex items-center justify-between p-4 rounded-xl border bg-card">
                            <div className="space-y-0.5">
                                <FormLabel className="text-base font-semibold">
                                    <Trans>Proxy Strategy</Trans>
                                </FormLabel>
                                <FormDescription>
                                    <Trans>Choose how this platform handles proxy connections.</Trans>
                                </FormDescription>
                            </div>
                            <Select
                                value={isInherited ? "inherit" : "custom"}
                                onValueChange={(v) => {
                                    if (v === "inherit") {
                                        field.onChange(null);
                                    } else {
                                        // Initialize with disabled proxy if coming from null
                                        field.onChange(JSON.stringify({ enabled: false, use_system_proxy: false }));
                                    }
                                }}
                            >
                                <FormControl>
                                    <SelectTrigger className="w-[180px]">
                                        <SelectValue />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="inherit"><Trans>Global Default</Trans></SelectItem>
                                    <SelectItem value="custom"><Trans>Custom Configuration</Trans></SelectItem>
                                </SelectContent>
                            </Select>
                        </div>

                        {!isInherited && (
                            <div className="animate-in fade-in-50 slide-in-from-top-2">
                                <ProxyConfigSettings
                                    value={field.value}
                                    onChange={field.onChange}
                                />
                            </div>
                        )}
                        <FormMessage />
                    </FormItem>
                );
            }}
        />
    );
}
