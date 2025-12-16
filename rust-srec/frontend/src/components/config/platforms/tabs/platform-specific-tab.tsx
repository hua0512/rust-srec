import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { Textarea } from '@/components/ui/textarea';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Boxes } from 'lucide-react';
import { useState, useEffect } from 'react';

interface PlatformSpecificTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function PlatformSpecificTab({
  form,
  basePath,
}: PlatformSpecificTabProps) {
  const fieldName = basePath
    ? `${basePath}.platform_specific_config`
    : 'platform_specific_config';

  return (
    <Card className="border-dashed shadow-none">
      <CardHeader className="pb-3">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
            <Boxes className="w-5 h-5" />
          </div>
          <CardTitle className="text-lg">
            <Trans>Platform Specific Configuration</Trans>
          </CardTitle>
        </div>
      </CardHeader>
      <CardContent>
        <FormField
          control={form.control}
          name={fieldName}
          render={({ field }) => {
            // Local state for the text area to allow typing invalid JSON
            const [text, setText] = useState('');
            const [error, setError] = useState<string | null>(null);

            // Sync from form state to local text state
            useEffect(() => {
              const val = field.value;
              if (val === null || val === undefined) {
                setText('');
              } else if (typeof val === 'object') {
                setText(JSON.stringify(val, null, 2));
              } else {
                setText(String(val));
              }
            }, [field.value]);

            const handleChange = (
              e: React.ChangeEvent<HTMLTextAreaElement>,
            ) => {
              const newVal = e.target.value;
              setText(newVal);

              if (!newVal.trim()) {
                field.onChange(null);
                setError(null);
                return;
              }

              try {
                const parsed = JSON.parse(newVal);
                field.onChange(parsed);
                setError(null);
              } catch (err) {
                // If invalid JSON, don't update form value (keep it as previous valid or null?)
                // Or maybe we need to allow saving string? But schemas say object/any.
                // Usually we want to block save or show error.
                // Here we just set error state. Form value isn't updated to invalid string if schema expects object.
                // But schema is z.any(). Backend expects Option<String> (JSON string).
                // Config adapter might strict parse.
                // Let's assume we must emit valid object.
                setError((err as Error).message);
              }
            };

            return (
              <FormItem>
                <FormControl>
                  <div className="space-y-2">
                    <Textarea
                      value={text}
                      onChange={handleChange}
                      className="font-mono text-sm min-h-[300px]"
                      placeholder="{ ... }"
                    />
                    {error && (
                      <p className="text-sm font-medium text-destructive">
                        Invalid JSON: {error}
                      </p>
                    )}
                  </div>
                </FormControl>
                <FormDescription>
                  <Trans>
                    Raw JSON configuration specific to this platform.
                  </Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            );
          }}
        />
      </CardContent>
    </Card>
  );
}
