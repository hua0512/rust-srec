import { UseFormReturn, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Link as RouterLink } from '@tanstack/react-router';
import { ArrowLeft, User, Link, Sparkles, Loader2 } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { motion } from 'motion/react';
import { Checkbox } from '@/components/ui/checkbox';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface StreamerMetaFormProps {
  form: UseFormReturn<any>;
  isUpdating?: boolean;
  title: React.ReactNode;
  onBack?: () => void;
  fieldsDisabled?: boolean;
  hideIdentityFields?: boolean;
  onAutofillName?: () => void;
  isAutofilling?: boolean;
  children?: React.ReactNode;
}

export function StreamerMetaForm({
  form,
  isUpdating = false,
  title,
  onBack,
  fieldsDisabled = false,
  hideIdentityFields = false,
  onAutofillName,
  isAutofilling = false,
  children,
}: StreamerMetaFormProps) {
  const url = useWatch({
    control: form.control,
    name: 'url',
  });

  return (
    <motion.div
      initial={{ opacity: 0, x: -20 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ duration: 0.4 }}
    >
      <Card className="border-border/40 shadow-sm bg-card/80 backdrop-blur-sm sticky top-6">
        <CardHeader className="pb-6 border-b border-border/40 bg-muted/10">
          <div className="flex items-center gap-4">
            <Button
              variant="ghost"
              size="icon"
              className="h-9 w-9 -ml-2 text-muted-foreground/70 hover:text-foreground hover:bg-background/50 rounded-full"
              onClick={onBack}
              disabled={isUpdating}
            >
              {isUpdating ? (
                <RouterLink to="/streamers">
                  <ArrowLeft className="h-5 w-5" />
                </RouterLink>
              ) : (
                <ArrowLeft className="h-5 w-5" />
              )}
            </Button>
            <div className="flex flex-col gap-0.5">
              <CardTitle className="text-lg font-semibold tracking-tight">
                <Trans>Streamer Details</Trans>
              </CardTitle>
              <CardDescription className="text-xs font-normal text-muted-foreground/80">
                {title}
              </CardDescription>
            </div>
            <div className="ml-auto p-2 rounded-xl bg-background/50 border border-border/50 shadow-sm text-primary">
              <User className="w-5 h-5" />
            </div>
          </div>
        </CardHeader>

        <CardContent className="space-y-6 p-6">
          {!hideIdentityFields && (
            <>
              <FormField
                control={form.control}
                name="url"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-medium ml-1">
                      <Trans>URL</Trans>
                    </FormLabel>
                    <FormControl>
                      <div className="relative">
                        <Link className="absolute left-3 top-3.5 h-4 w-4 text-muted-foreground" />
                        <Input
                          {...field}
                          placeholder="https://..."
                          className="h-11 pl-9 bg-muted/30 border-muted-foreground/20 focus:bg-background transition-all font-mono text-xs"
                          disabled={fieldsDisabled}
                        />
                      </div>
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />

              <FormField
                control={form.control}
                name="name"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-medium ml-1">
                      <Trans>Name</Trans>
                    </FormLabel>
                    <FormControl>
                      <div className="flex gap-2">
                        <Input
                          {...field}
                          placeholder={t`e.g. My Favorite Streamer`}
                          className="h-11 bg-muted/30 border-muted-foreground/20 focus:bg-background transition-all"
                          disabled={fieldsDisabled}
                        />
                        {onAutofillName && (
                          <TooltipProvider>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  type="button"
                                  variant="outline"
                                  size="icon"
                                  className="h-11 w-11 shrink-0 bg-muted/30 border-muted-foreground/20 hover:bg-background transition-all"
                                  onClick={onAutofillName}
                                  disabled={
                                    fieldsDisabled || isAutofilling || !url
                                  }
                                >
                                  {isAutofilling ? (
                                    <Loader2 className="h-4 w-4 animate-spin text-primary" />
                                  ) : (
                                    <Sparkles className="h-4 w-4 text-primary" />
                                  )}
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>
                                <p>
                                  <Trans>Autofill name from URL</Trans>
                                </p>
                              </TooltipContent>
                            </Tooltip>
                          </TooltipProvider>
                        )}
                      </div>
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </>
          )}

          <FormField
            control={form.control}
            name="priority"
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-medium ml-1">
                  <Trans>Priority</Trans>
                </FormLabel>
                <Select onValueChange={field.onChange} value={field.value}>
                  <FormControl>
                    <SelectTrigger className="h-11 bg-muted/30 border-muted-foreground/20 focus:bg-background transition-all">
                      <SelectValue placeholder={t`Select priority`} />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="HIGH">
                      <Trans>High</Trans>
                    </SelectItem>
                    <SelectItem value="NORMAL">
                      <Trans>Normal</Trans>
                    </SelectItem>
                    <SelectItem value="LOW">
                      <Trans>Low</Trans>
                    </SelectItem>
                  </SelectContent>
                </Select>
                <FormMessage />
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name="enabled"
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/50 p-4 bg-muted/20 shadow-sm">
                <div className="space-y-0.5">
                  <FormLabel className="text-sm font-medium">
                    <Trans>Active</Trans>
                  </FormLabel>
                  <FormDescription className="text-xs">
                    <Trans>Monitor this streamer</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Checkbox
                    checked={field.value}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />
          {children}
        </CardContent>
      </Card>
    </motion.div>
  );
}
