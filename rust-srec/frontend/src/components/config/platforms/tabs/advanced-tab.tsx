import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '../../../ui/form';
import { Textarea } from '../../../ui/textarea';
import { Trans } from '@lingui/react/macro';
import { Terminal } from 'lucide-react';
import { UseFormReturn } from 'react-hook-form';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';

interface AdvancedTabProps {
  form: UseFormReturn<any>;
  basePath?: string;
}

export function AdvancedTab({ form, basePath }: AdvancedTabProps) {
  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-slate-500/10 text-slate-600 dark:text-slate-400">
            <Terminal className="w-5 h-5" />
          </div>
          <div className="space-y-1">
            <CardTitle className="text-lg">
              <Trans>Advanced Configuration</Trans>
            </CardTitle>
            <CardDescription>
              <Trans>Raw JSON configurations for advanced users.</Trans>
            </CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent>
        <Accordion type="multiple" className="w-full">
          <AccordionItem value="platform-specific" className="border-b-0">
            <AccordionTrigger className="hover:no-underline rounded-lg hover:bg-muted/50 px-4">
              <span className="font-semibold">
                <Trans>Platform Specific Config</Trans>
              </span>
            </AccordionTrigger>
            <AccordionContent className="px-4 pt-4 pb-4">
              <FormField
                control={form.control}
                name={
                  basePath
                    ? `${basePath}.platform_specific_config`
                    : 'platform_specific_config'
                }
                render={({ field }) => (
                  <FormItem>
                    <FormControl>
                      <Textarea
                        placeholder='{"output_folder": "./custom"}'
                        className="font-mono text-xs min-h-[150px] bg-muted/30"
                        {...field}
                        value={field.value ?? ''}
                        onChange={(e) => field.onChange(e.target.value || null)}
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>Legacy JSON blob.</Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </AccordionContent>
          </AccordionItem>

          <AccordionItem value="retry-policy" className="border-b-0">
            <AccordionTrigger className="hover:no-underline rounded-lg hover:bg-muted/50 px-4">
              <span className="font-semibold">
                <Trans>Retry Policy</Trans>
              </span>
            </AccordionTrigger>
            <AccordionContent className="px-4 pt-4 pb-4">
              <FormField
                control={form.control}
                name={
                  basePath
                    ? `${basePath}.download_retry_policy`
                    : 'download_retry_policy'
                }
                render={({ field }) => (
                  <FormItem>
                    <FormControl>
                      <Textarea
                        {...field}
                        value={field.value ?? ''}
                        onChange={(e) => field.onChange(e.target.value || null)}
                        className="font-mono text-xs min-h-[100px] bg-muted/30"
                        placeholder='{"max_retries": 10, "retry_delay": 10}'
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </AccordionContent>
          </AccordionItem>

          <AccordionItem value="event-hooks" className="border-b-0">
            <AccordionTrigger className="hover:no-underline rounded-lg hover:bg-muted/50 px-4">
              <span className="font-semibold">
                <Trans>Event Hooks</Trans>
              </span>
            </AccordionTrigger>
            <AccordionContent className="px-4 pt-4 pb-4">
              <FormField
                control={form.control}
                name={basePath ? `${basePath}.event_hooks` : 'event_hooks'}
                render={({ field }) => (
                  <FormItem>
                    <FormControl>
                      <Textarea
                        {...field}
                        value={field.value ?? ''}
                        onChange={(e) => field.onChange(e.target.value || null)}
                        className="font-mono text-xs min-h-[100px] bg-muted/30"
                        placeholder='{"on_start": "cmd"}'
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </AccordionContent>
          </AccordionItem>
        </Accordion>
      </CardContent>
    </Card>
  );
}
