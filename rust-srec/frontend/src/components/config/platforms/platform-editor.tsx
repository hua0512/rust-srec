import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import { Form } from '@/components/ui/form';
import { Button } from '@/components/ui/button';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Settings, Cookie, Shield, Code, Filter, Save, Loader2 } from 'lucide-react';
import { PlatformConfigSchema } from '@/api/schemas';
import { GeneralTab } from './tabs/general-tab';
import { StreamSelectionTab } from './tabs/stream-selection-tab';
import { AuthTab } from './tabs/auth-tab';
import { AdvancedTab } from './tabs/advanced-tab';
import { ProxyTab } from './tabs/proxy-tab';

const EditPlatformSchema = PlatformConfigSchema.partial();
export type EditPlatformFormValues = z.infer<typeof EditPlatformSchema>;

interface PlatformEditorProps {
    platform: z.infer<typeof PlatformConfigSchema>;
    onSubmit: (data: EditPlatformFormValues) => void;
    isUpdating: boolean;
}

export function PlatformEditor({ platform, onSubmit, isUpdating }: PlatformEditorProps) {
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

    return (
        <Form {...form}>
            <form onSubmit={form.handleSubmit(onSubmit)} className="pb-24">
                <motion.div
                    initial={{ opacity: 0, y: 10 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ duration: 0.3 }}
                >
                    <Tabs defaultValue="general" className="w-full">
                        <div className="sticky top-0 z-10 bg-background/80 backdrop-blur-xl pb-4 -mx-4 px-4 md:-mx-8 md:px-8">
                            <TabsList className="flex flex-wrap h-auto w-full justify-start gap-2 bg-transparent p-0">
                                <TabsTrigger
                                    value="general"
                                    className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-card hover:bg-muted/60 transition-all shadow-sm rounded-lg"
                                >
                                    <Settings className="w-4 h-4" />
                                    <span className="whitespace-nowrap font-medium"><Trans>General</Trans></span>
                                </TabsTrigger>
                                <TabsTrigger
                                    value="stream-selection"
                                    className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-card hover:bg-muted/60 transition-all shadow-sm rounded-lg"
                                >
                                    <Filter className="w-4 h-4" />
                                    <span className="whitespace-nowrap font-medium"><Trans>Stream Selection</Trans></span>
                                </TabsTrigger>
                                <TabsTrigger
                                    value="auth"
                                    className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-card hover:bg-muted/60 transition-all shadow-sm rounded-lg"
                                >
                                    <Cookie className="w-4 h-4" />
                                    <span className="whitespace-nowrap font-medium"><Trans>Authentication</Trans></span>
                                </TabsTrigger>
                                <TabsTrigger
                                    value="proxy"
                                    className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-card hover:bg-muted/60 transition-all shadow-sm rounded-lg"
                                >
                                    <Shield className="w-4 h-4" />
                                    <span className="whitespace-nowrap font-medium"><Trans>Proxy</Trans></span>
                                </TabsTrigger>
                                <TabsTrigger
                                    value="advanced"
                                    className="gap-2 px-4 py-2.5 h-10 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground border bg-card hover:bg-muted/60 transition-all shadow-sm rounded-lg"
                                >
                                    <Code className="w-4 h-4" />
                                    <span className="whitespace-nowrap font-medium"><Trans>Advanced</Trans></span>
                                </TabsTrigger>
                            </TabsList>
                        </div>

                        <div className="mt-2">
                            <TabsContent value="general" className="space-y-6 animate-in fade-in-50 duration-300 mt-0">
                                <GeneralTab form={form} />
                            </TabsContent>

                            <TabsContent value="stream-selection" className="space-y-6 animate-in fade-in-50 duration-300 mt-0">
                                <StreamSelectionTab form={form} />
                            </TabsContent>

                            <TabsContent value="auth" className="space-y-4 animate-in fade-in-50 duration-300 mt-0">
                                <AuthTab form={form} />
                            </TabsContent>

                            <TabsContent value="proxy" className="animate-in fade-in-50 duration-300 mt-0">
                                <ProxyTab form={form} />
                            </TabsContent>

                            <TabsContent value="advanced" className="animate-in fade-in-50 duration-300 mt-0">
                                <AdvancedTab form={form} />
                            </TabsContent>
                        </div>
                    </Tabs>
                </motion.div>

                {/* Floating Save Button */}
                {form.formState.isDirty && (
                    <div className="fixed bottom-8 right-8 z-50 animate-in fade-in slide-in-from-bottom-4 duration-300">
                        <Button
                            type="submit"
                            disabled={isUpdating}
                            size="lg"
                            className="shadow-2xl shadow-primary/40 hover:shadow-primary/50 transition-all hover:scale-105 active:scale-95 rounded-full px-8 h-14 bg-gradient-to-r from-primary to-primary/90 text-base font-semibold"
                        >
                            {isUpdating ? (
                                <Loader2 className="w-5 h-5 mr-2 animate-spin" />
                            ) : (
                                <Save className="w-5 h-5 mr-2" />
                            )}
                            {isUpdating ? <Trans>Saving...</Trans> : <Trans>Save Changes</Trans>}
                        </Button>
                    </div>
                )}
            </form>
        </Form>
    );
}
