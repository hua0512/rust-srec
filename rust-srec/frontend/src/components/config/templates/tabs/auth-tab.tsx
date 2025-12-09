import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '../../../ui/form';

import { Textarea } from '../../../ui/textarea';
import { Trans } from '@lingui/react/macro';
import { Cookie } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { UpdateTemplateRequestSchema } from '../../../../api/schemas';

type EditTemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;

interface AuthTabProps {
    form: UseFormReturn<EditTemplateFormValues>;
}

export function AuthTab({ form }: AuthTabProps) {
    return (
        <div className="grid gap-6">
            <FormField
                control={form.control}
                name="cookies"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel className="flex items-center gap-2">
                            <Cookie className="w-4 h-4" /> <Trans>Cookies</Trans>
                        </FormLabel>
                        <FormControl>
                            <Textarea
                                {...field}
                                value={field.value ?? ''}
                                onChange={(e) => field.onChange(e.target.value || null)}
                                placeholder="key=value; key2=value2;"
                                className="min-h-[150px] font-mono text-xs"
                            />
                        </FormControl>
                        <FormDescription>
                            <Trans>Cookies to use regardless of the platform.</Trans>
                        </FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />
        </div>
    );
}
