import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormMessage,
} from '../../../ui/form';
import { Textarea } from '../../../ui/textarea';
import { Trans } from '@lingui/react/macro';
import { Cookie } from 'lucide-react';
import { Separator } from '../../../ui/separator';
import { UseFormReturn } from 'react-hook-form';



interface AuthTabProps {
    form: UseFormReturn<any>;
    basePath?: string;
}

export function AuthTab({ form, basePath }: AuthTabProps) {
    return (
        <div className="rounded-xl border bg-card text-card-foreground shadow-sm p-6 space-y-4">
            <div className="flex items-center gap-3 mb-2">
                <div className="p-2 bg-orange-500/10 text-orange-500 rounded-lg">
                    <Cookie className="w-5 h-5" />
                </div>
                <div>
                    <h3 className="font-semibold"><Trans>Authentication Cookies</Trans></h3>
                    <p className="text-sm text-muted-foreground"><Trans>Required for premium/login-only content.</Trans></p>
                </div>
            </div>
            <Separator />
            <FormField
                control={form.control}
                name={basePath ? `${basePath}.cookies` : "cookies"}
                render={({ field }) => (
                    <FormItem>
                        <FormControl>
                            <Textarea
                                placeholder="key=value; key2=value2"
                                className="font-mono text-xs min-h-[200px]"
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                            />
                        </FormControl>
                        <FormDescription><Trans>Paste your Netscape formatted cookies or raw key=value string here.</Trans></FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />
        </div>
    );
}
