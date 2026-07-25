import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';

import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { Type } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { UpdateTemplateRequestSchema } from '@/api/schemas';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { CONFIG_INPUT } from '@/components/config/shared/config-field';
import {
  CONFIG_DESCRIPTION,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

type EditTemplateFormValues = z.input<typeof UpdateTemplateRequestSchema>;

interface GeneralTabProps {
  form: UseFormReturn<EditTemplateFormValues>;
}

export function GeneralTab({ form }: GeneralTabProps) {
  const { i18n } = useLingui();
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
              <FormItem className="space-y-2">
                <ConfigFieldLabel>
                  <Trans>Template Name</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    {...field}
                    value={field.value ?? ''}
                    placeholder={i18n._(msg`My Template`)}
                    className={CONFIG_INPUT}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
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
