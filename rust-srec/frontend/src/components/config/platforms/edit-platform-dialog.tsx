import { useState } from 'react';
import { useQueryClient, useMutation } from '@tanstack/react-query';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '../../ui/dialog';
import { Button } from '../../ui/button';
import { Form } from '../../ui/form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../ui/tabs';
import {
  Settings,
  Cookie,
  Shield,
  Code,
  Pencil,
  Loader2,
  Filter,
} from 'lucide-react';
import { PlatformConfigSchema } from '../../../api/schemas';
import { updatePlatformConfig } from '@/server/functions';
import { GeneralTab } from './tabs/general-tab';
import { StreamSelectionTab } from './tabs/stream-selection-tab';
import { AuthTab } from './tabs/auth-tab';
import { AdvancedTab } from './tabs/advanced-tab';
import { ProxyTab } from './tabs/proxy-tab';

const EditPlatformSchema = PlatformConfigSchema.partial();
export type EditPlatformFormValues = z.infer<typeof EditPlatformSchema>;

interface EditPlatformDialogProps {
  platform: z.infer<typeof PlatformConfigSchema>;
  trigger?: React.ReactNode;
}

export function EditPlatformDialog({
  platform,
  trigger,
}: EditPlatformDialogProps) {
  const [open, setOpen] = useState(false);
  const queryClient = useQueryClient();

  const form = useForm<EditPlatformFormValues>({
    resolver: zodResolver(EditPlatformSchema),
    defaultValues: {
      fetch_delay_ms: platform.fetch_delay_ms,
      download_delay_ms: platform.download_delay_ms,
      record_danmu: platform.record_danmu,
      cookies: platform.cookies,
      platform_specific_config: platform.platform_specific_config,
      proxy_config: platform.proxy_config,
      output_folder: platform.output_folder,
      output_filename_template: platform.output_filename_template,
      download_engine: platform.download_engine,
      stream_selection_config: platform.stream_selection_config,
      output_file_format: platform.output_file_format,
      min_segment_size_bytes: platform.min_segment_size_bytes,
      max_download_duration_secs: platform.max_download_duration_secs,
      max_part_size_bytes: platform.max_part_size_bytes,
      download_retry_policy: platform.download_retry_policy,
      event_hooks: platform.event_hooks,
    },
  });

  const updateMutation = useMutation({
    mutationFn: (data: EditPlatformFormValues) =>
      updatePlatformConfig({ data: { id: platform.id, data } }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['config', 'platforms'] });
      toast.success(`Updated ${platform.name} configuration`);
      setOpen(false);
    },
    onError: (error) => {
      toast.error(`Failed to update platform: ${error.message}`);
    },
  });

  function onSubmit(data: EditPlatformFormValues) {
    updateMutation.mutate(data);
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        {trigger || (
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 hover:bg-primary/10 hover:text-primary transition-colors"
          >
            <Pencil className="h-4 w-4" />
          </Button>
        )}
      </DialogTrigger>
      <DialogContent className="max-w-2xl max-h-[85vh] overflow-hidden flex flex-col p-0 gap-0">
        <DialogHeader className="p-6 pb-2">
          <DialogTitle className="flex items-center gap-2 text-xl">
            <div className="p-2 bg-primary/10 text-primary rounded-lg">
              <Settings className="w-5 h-5" />
            </div>
            <Trans>Edit {platform.name}</Trans>
          </DialogTitle>
          <DialogDescription className="pl-11">
            <Trans>
              Configure platform behavior, authentication, and advanced
              settings.
            </Trans>
          </DialogDescription>
        </DialogHeader>

        <Form {...form}>
          <form
            onSubmit={form.handleSubmit(onSubmit)}
            className="flex-1 overflow-hidden flex flex-col"
          >
            <div className="flex-1 overflow-y-auto p-6 pt-2">
              <Tabs defaultValue="general" className="w-full">
                <TabsList className="flex flex-wrap h-auto w-full justify-start gap-2 bg-transparent p-0 mb-4">
                  <TabsTrigger
                    value="general"
                    className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-md flex-grow md:flex-grow-0 ring-offset-background focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <Settings className="w-4 h-4" />
                    <span className="whitespace-nowrap">
                      <Trans>General</Trans>
                    </span>
                  </TabsTrigger>
                  <TabsTrigger
                    value="stream-selection"
                    className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-md flex-grow md:flex-grow-0 ring-offset-background focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <Filter className="w-4 h-4" />
                    <span className="whitespace-nowrap">
                      <Trans>Stream Selection</Trans>
                    </span>
                  </TabsTrigger>
                  <TabsTrigger
                    value="auth"
                    className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-md flex-grow md:flex-grow-0 ring-offset-background focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <Cookie className="w-4 h-4" />
                    <span className="whitespace-nowrap">
                      <Trans>Authentication</Trans>
                    </span>
                  </TabsTrigger>
                  <TabsTrigger
                    value="proxy"
                    className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-md flex-grow md:flex-grow-0 ring-offset-background focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <Shield className="w-4 h-4" />
                    <span className="whitespace-nowrap">
                      <Trans>Proxy</Trans>
                    </span>
                  </TabsTrigger>
                  <TabsTrigger
                    value="advanced"
                    className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-md flex-grow md:flex-grow-0 ring-offset-background focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    <Code className="w-4 h-4" />
                    <span className="whitespace-nowrap">
                      <Trans>Advanced</Trans>
                    </span>
                  </TabsTrigger>
                </TabsList>

                <div className="mt-2">
                  <TabsContent
                    value="general"
                    className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300"
                  >
                    <GeneralTab form={form} />
                  </TabsContent>

                  <TabsContent
                    value="stream-selection"
                    className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300"
                  >
                    <StreamSelectionTab form={form} />
                  </TabsContent>

                  <TabsContent
                    value="auth"
                    className="space-y-4 animate-in fade-in-50 slide-in-from-left-2 duration-300"
                  >
                    <AuthTab form={form} />
                  </TabsContent>

                  <TabsContent
                    value="proxy"
                    className="animate-in fade-in-50 slide-in-from-left-2 duration-300"
                  >
                    <ProxyTab form={form} />
                  </TabsContent>

                  <TabsContent
                    value="advanced"
                    className="animate-in fade-in-50 slide-in-from-left-2 duration-300"
                  >
                    <AdvancedTab form={form} />
                  </TabsContent>
                </div>
              </Tabs>
            </div>

            <DialogFooter className="p-6 pt-2 border-t mt-auto bg-muted/30">
              <Button
                type="button"
                variant="ghost"
                onClick={() => setOpen(false)}
              >
                <Trans>Cancel</Trans>
              </Button>
              <Button
                type="submit"
                disabled={updateMutation.isPending}
                className="bg-primary/90 hover:bg-primary"
              >
                {updateMutation.isPending && (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                )}
                <Trans>Save Changes</Trans>
              </Button>
            </DialogFooter>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  );
}
