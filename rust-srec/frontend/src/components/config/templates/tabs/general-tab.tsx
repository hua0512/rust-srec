import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';

import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { Type } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { UpdateTemplateRequestSchema } from '../../../../api/schemas';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';

type EditTemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;

interface GeneralTabProps {
  form: UseFormReturn<EditTemplateFormValues>;
}

export function GeneralTab({ form }: GeneralTabProps) {
  return (
    <div className="grid gap-6">
      {/* Template Information */}
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader>
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-primary/10 text-primary">
              <Type className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Template Information</Trans>
              </CardTitle>
              <p className="text-sm text-muted-foreground">
                <Trans>Basic details for this configuration template.</Trans>
              </p>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <FormField
            control={form.control}
            name="name"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Template Name</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    {...field}
                    value={field.value ?? ''}
                    placeholder="My Template"
                    className="bg-background"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>A unique name for this configuration template.</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>
    </div>
  );
}
