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
import { Form, FormControl, FormItem } from '../../ui/form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../ui/tabs';
import { Settings, Cookie, Shield, Code, Loader2, Filter, LayoutTemplate, Plus, Server } from 'lucide-react';
import { TemplateSchema, UpdateTemplateRequestSchema } from '../../../api/schemas';
import { configApi } from '../../../api/endpoints';
import { GeneralTab } from './tabs/general-tab';
import { StreamSelectionTab } from './tabs/stream-selection-tab';
import { AuthTab } from './tabs/auth-tab';
import { AdvancedTab } from './tabs/advanced-tab';
import { ProxyTab } from './tabs/proxy-tab';
import { EngineOverridesTab } from './tabs/engine-overrides-tab';
import { PlatformOverridesTab } from './tabs/platform-overrides-tab';

type EditTemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;

interface EditTemplateDialogProps {
    template?: z.infer<typeof TemplateSchema>;
    trigger?: React.ReactNode;
}

export function EditTemplateDialog({ template, trigger }: EditTemplateDialogProps) {
    const [open, setOpen] = useState(false);
    const queryClient = useQueryClient();
    const isEditing = !!template;

    const form = useForm<EditTemplateFormValues>({
        resolver: zodResolver(UpdateTemplateRequestSchema),
        defaultValues: template ? {
            name: template.name,
            output_folder: template.output_folder,
            output_filename_template: template.output_filename_template,
            output_file_format: template.output_file_format,
            max_bitrate: template.max_bitrate,
            min_segment_size_bytes: template.min_segment_size_bytes,
            max_download_duration_secs: template.max_download_duration_secs,
            max_part_size_bytes: template.max_part_size_bytes,
            record_danmu: template.record_danmu,
            cookies: template.cookies,
            platform_overrides: template.platform_overrides,
            download_retry_policy: template.download_retry_policy,
            danmu_sampling_config: template.danmu_sampling_config,
            download_engine: template.download_engine,
            engines_override: template.engines_override,
            proxy_config: template.proxy_config,
            event_hooks: template.event_hooks,
            stream_selection_config: template.stream_selection_config,
        } : {
            name: "",
            output_folder: null,
            record_danmu: null,
        },
    });

    const mutation = useMutation({
        mutationFn: (data: EditTemplateFormValues) => {
            if (isEditing && template) {
                return configApi.updateTemplate(template.id, data);
            } else {
                // Ensure name is present for creation
                if (!data.name) throw new Error("Name is required");
                return configApi.createTemplate(data as any);
            }
        },
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['templates'] });
            toast.success(isEditing ? `Updated template` : `Created template`);
            setOpen(false);
            if (!isEditing) form.reset();
        },
        onError: (error) => {
            toast.error(`Failed to save template: ${error.message}`);
        },
    });

    function onSubmit(data: EditTemplateFormValues) {
        mutation.mutate(data);
    }

    return (
        <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild>
                {trigger || (
                    <Button variant="outline" className="gap-2">
                        <Plus className="w-4 h-4" />
                        <Trans>Create Template</Trans>
                    </Button>
                )}
            </DialogTrigger>
            <DialogContent className="max-w-2xl max-h-[85vh] overflow-hidden flex flex-col p-0 gap-0">
                <DialogHeader className="p-6 pb-2">
                    <DialogTitle className="flex items-center gap-2 text-xl">
                        <div className="p-2 bg-primary/10 text-primary rounded-lg">
                            <LayoutTemplate className="w-5 h-5" />
                        </div>
                        {isEditing ? <Trans>Edit {template.name}</Trans> : <Trans>Create Template</Trans>}
                    </DialogTitle>
                    <DialogDescription className="pl-11">
                        <Trans>Configure reusable settings for downloads.</Trans>
                    </DialogDescription>
                </DialogHeader>

                <Form {...form}>
                    <form onSubmit={form.handleSubmit(onSubmit)} className="flex-1 overflow-hidden flex flex-col">
                        <div className="flex-1 overflow-y-auto p-6 pt-2">
                            <Tabs defaultValue="general" className="w-full">
                                <FormItem className="w-full">
                                    <FormControl>
                                        <div className="w-full overflow-x-auto pb-2 -mb-2">
                                            <TabsList className="flex flex-wrap h-auto w-fit justify-start gap-2 bg-transparent p-0 mb-4">
                                                <TabsTrigger value="general" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Settings className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>General</Trans></span>
                                                </TabsTrigger>
                                                <TabsTrigger value="stream-selection" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Filter className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>Stream Selection</Trans></span>
                                                </TabsTrigger>
                                                <TabsTrigger value="auth" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Cookie className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>Authentication</Trans></span>
                                                </TabsTrigger>
                                                <TabsTrigger value="engine-overrides" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Server className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>Engine Overrides</Trans></span>
                                                </TabsTrigger>
                                                <TabsTrigger value="platform-overrides" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Settings className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>Platform Overrides</Trans></span>
                                                </TabsTrigger>
                                                <TabsTrigger value="proxy" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Shield className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>Proxy</Trans></span>
                                                </TabsTrigger>
                                                <TabsTrigger value="advanced" className="gap-2 px-4 py-2 h-9 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-muted/40 hover:bg-muted/60 transition-colors shadow-sm rounded-full ring-offset-background focus-visible:ring-2 focus-visible:ring-ring">
                                                    <Code className="w-4 h-4" />
                                                    <span className="whitespace-nowrap"><Trans>Advanced</Trans></span>
                                                </TabsTrigger>
                                            </TabsList>
                                        </div>
                                    </FormControl>
                                </FormItem>

                                <div className="mt-2 text-sm">
                                    <TabsContent value="general" className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <GeneralTab form={form} />
                                    </TabsContent>

                                    <TabsContent value="stream-selection" className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <StreamSelectionTab form={form} />
                                    </TabsContent>

                                    <TabsContent value="auth" className="space-y-4 animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <AuthTab form={form} />
                                    </TabsContent>

                                    <TabsContent value="engine-overrides" className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <EngineOverridesTab form={form} />
                                    </TabsContent>

                                    <TabsContent value="platform-overrides" className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <PlatformOverridesTab form={form} />
                                    </TabsContent>

                                    <TabsContent value="proxy" className="animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <ProxyTab form={form} />
                                    </TabsContent>

                                    <TabsContent value="advanced" className="animate-in fade-in-50 slide-in-from-left-2 duration-300">
                                        <AdvancedTab form={form} />
                                    </TabsContent>
                                </div>
                            </Tabs>
                        </div>

                        <DialogFooter className="p-6 pt-2 border-t mt-auto bg-muted/30">
                            <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
                                <Trans>Cancel</Trans>
                            </Button>
                            <Button type="submit" disabled={mutation.isPending} className="bg-primary/90 hover:bg-primary">
                                {mutation.isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                                <Trans>{isEditing ? 'Save Changes' : 'Create Template'}</Trans>
                            </Button>
                        </DialogFooter>
                    </form>
                </Form>
            </DialogContent>
        </Dialog>
    );
}
