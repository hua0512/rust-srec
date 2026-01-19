import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Button } from '@/components/ui/button';
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Loader2, Play, Settings2, ChevronDown } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { motion, AnimatePresence } from 'motion/react';
import { useState } from 'react';
import { cn } from '@/lib/utils';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';

const getSingleUrlSchema = (i18n: any) =>
  z.object({
    url: z
      .url(i18n._(msg`Please enter a valid URL`))
      .min(1, i18n._(msg`URL is required`)),
    cookies: z.string().optional(),
  });

const getBatchUrlSchema = (i18n: any) =>
  z.object({
    urls: z.string().min(1, i18n._(msg`Please enter at least one URL`)),
    cookies: z.string().optional(),
  });

type SingleUrlFormValues = z.infer<ReturnType<typeof getSingleUrlSchema>>;
type BatchUrlFormValues = z.infer<ReturnType<typeof getBatchUrlSchema>>;

export interface UrlInputFormProps {
  onSubmitSingle: (data: { url: string; cookies?: string }) => void;
  onSubmitBatch: (data: { urls: string[]; cookies?: string }) => void;
  isLoading?: boolean;
}

export function UrlInputForm({
  onSubmitSingle,
  onSubmitBatch,
  isLoading = false,
}: UrlInputFormProps) {
  const { i18n } = useLingui();
  const [mode, setMode] = useState<'single' | 'batch'>('single');
  const [showAdvanced, setShowAdvanced] = useState(false);

  const singleForm = useForm<SingleUrlFormValues>({
    resolver: zodResolver(getSingleUrlSchema(i18n)),
    defaultValues: {
      url: '',
      cookies: '',
    },
  });

  const batchForm = useForm<BatchUrlFormValues>({
    resolver: zodResolver(getBatchUrlSchema(i18n)),
    defaultValues: {
      urls: '',
      cookies: '',
    },
  });

  const handleSingleSubmit = (data: SingleUrlFormValues) => {
    onSubmitSingle({
      url: data.url,
      cookies: data.cookies || undefined,
    });
  };

  const handleBatchSubmit = (data: BatchUrlFormValues) => {
    const urlList = data.urls
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.length > 0);

    onSubmitBatch({
      urls: urlList,
      cookies: data.cookies || undefined,
    });
  };

  return (
    <div className="w-full max-w-md mx-auto space-y-6">
      {/* Custom Tabs */}
      <div className="grid grid-cols-2 p-1 bg-muted/40 rounded-xl border border-border/40 backdrop-blur-sm">
        <button
          onClick={() => setMode('single')}
          className={cn(
            'relative flex items-center justify-center gap-2 py-2.5 text-sm font-medium rounded-lg transition-all duration-300',
            mode === 'single'
              ? 'text-primary-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground hover:bg-muted/50',
          )}
        >
          {mode === 'single' && (
            <motion.div
              layoutId="activeTab"
              className="absolute inset-0 bg-primary rounded-lg"
              transition={{ type: 'spring', bounce: 0.2, duration: 0.6 }}
            />
          )}
          <span className="relative z-10 flex items-center gap-2">
            <Trans>Single URL</Trans>
          </span>
        </button>
        <button
          onClick={() => setMode('batch')}
          className={cn(
            'relative flex items-center justify-center gap-2 py-2.5 text-sm font-medium rounded-lg transition-all duration-300',
            mode === 'batch'
              ? 'text-primary-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground hover:bg-muted/50',
          )}
        >
          {mode === 'batch' && (
            <motion.div
              layoutId="activeTab"
              className="absolute inset-0 bg-primary rounded-lg"
              transition={{ type: 'spring', bounce: 0.2, duration: 0.6 }}
            />
          )}
          <span className="relative z-10 flex items-center gap-2">
            <Trans>Batch URLs</Trans>
          </span>
        </button>
      </div>

      <AnimatePresence mode="wait">
        {mode === 'single' ? (
          <motion.div
            key="single"
            initial={{ opacity: 0, x: -20 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 20 }}
            transition={{ duration: 0.2 }}
          >
            <Form {...singleForm}>
              <form
                onSubmit={singleForm.handleSubmit(handleSingleSubmit)}
                className="space-y-4"
              >
                <FormField
                  control={singleForm.control}
                  name="url"
                  render={({ field }) => (
                    <FormItem>
                      <FormControl>
                        <div className="relative group">
                          <Input
                            placeholder="https://example.com/stream"
                            className="h-12 pl-4 pr-4 bg-background/50 border-input/60 focus:border-primary/50 focus:ring-primary/20 backdrop-blur-sm transition-all shadow-sm group-hover:border-primary/30"
                            {...field}
                            disabled={isLoading}
                          />
                        </div>
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <Collapsible open={showAdvanced} onOpenChange={setShowAdvanced}>
                  <CollapsibleTrigger asChild>
                    <Button
                      variant="ghost"
                      size="sm"
                      type="button"
                      className="w-full flex items-center justify-between text-muted-foreground hover:text-foreground h-9 mt-1"
                    >
                      <span className="flex items-center gap-2 text-xs uppercase tracking-wider font-semibold">
                        <Settings2 className="h-3.5 w-3.5" />
                        <Trans>Advanced Options</Trans>
                      </span>
                      <ChevronDown
                        className={cn(
                          'h-4 w-4 transition-transform duration-200',
                          showAdvanced && 'rotate-180',
                        )}
                      />
                    </Button>
                  </CollapsibleTrigger>
                  <CollapsibleContent className="space-y-4 pt-4">
                    <FormField
                      control={singleForm.control}
                      name="cookies"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel className="text-xs">Cookies</FormLabel>
                          <FormControl>
                            <Input
                              placeholder="key=value; key2=value2"
                              className="bg-background/50"
                              {...field}
                              disabled={isLoading}
                            />
                          </FormControl>
                        </FormItem>
                      )}
                    />
                  </CollapsibleContent>
                </Collapsible>

                <Button
                  type="submit"
                  className="w-full h-11 bg-gradient-to-r from-primary to-primary/90 hover:from-primary/90 hover:to-primary shadow-lg shadow-primary/20 transition-all duration-300 transform hover:scale-[1.02] active:scale-[0.98]"
                  disabled={isLoading}
                >
                  {isLoading ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      <Trans>Parsing...</Trans>
                    </>
                  ) : (
                    <>
                      <Play className="mr-2 h-4 w-4 fill-current" />
                      <Trans>Parse Stream</Trans>
                    </>
                  )}
                </Button>
              </form>
            </Form>
          </motion.div>
        ) : (
          <motion.div
            key="batch"
            initial={{ opacity: 0, x: 20 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -20 }}
            transition={{ duration: 0.2 }}
          >
            <Form {...batchForm}>
              <form
                onSubmit={batchForm.handleSubmit(handleBatchSubmit)}
                className="space-y-4"
              >
                <FormField
                  control={batchForm.control}
                  name="urls"
                  render={({ field }) => (
                    <FormItem>
                      <FormControl>
                        <Textarea
                          placeholder="https://example.com/stream1&#10;https://example.com/stream2"
                          className="min-h-[140px] font-mono text-sm bg-background/50 border-input/60 focus:border-primary/50 focus:ring-primary/20 backdrop-blur-sm transition-all shadow-sm leading-relaxed p-4"
                          {...field}
                          disabled={isLoading}
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <Collapsible open={showAdvanced} onOpenChange={setShowAdvanced}>
                  <CollapsibleTrigger asChild>
                    <Button
                      variant="ghost"
                      size="sm"
                      type="button"
                      className="w-full flex items-center justify-between text-muted-foreground hover:text-foreground h-9 mt-1"
                    >
                      <span className="flex items-center gap-2 text-xs uppercase tracking-wider font-semibold">
                        <Settings2 className="h-3.5 w-3.5" />
                        <Trans>Advanced Options</Trans>
                      </span>
                      <ChevronDown
                        className={cn(
                          'h-4 w-4 transition-transform duration-200',
                          showAdvanced && 'rotate-180',
                        )}
                      />
                    </Button>
                  </CollapsibleTrigger>
                  <CollapsibleContent className="space-y-4 pt-4">
                    <FormField
                      control={batchForm.control}
                      name="cookies"
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel className="text-xs">Cookies</FormLabel>
                          <FormControl>
                            <Input
                              placeholder="key=value; key2=value2"
                              className="bg-background/50"
                              {...field}
                              disabled={isLoading}
                            />
                          </FormControl>
                          <FormDescription>Applied to all URLs</FormDescription>
                        </FormItem>
                      )}
                    />
                  </CollapsibleContent>
                </Collapsible>

                <Button
                  type="submit"
                  className="w-full h-11 bg-gradient-to-r from-primary to-primary/90 hover:from-primary/90 hover:to-primary shadow-lg shadow-primary/20 transition-all duration-300 transform hover:scale-[1.02] active:scale-[0.98]"
                  disabled={isLoading}
                >
                  {isLoading ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      <Trans>Parsing...</Trans>
                    </>
                  ) : (
                    <>
                      <Play className="mr-2 h-4 w-4 fill-current" />
                      <Trans>Parse Batch</Trans>
                    </>
                  )}
                </Button>
              </form>
            </Form>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
