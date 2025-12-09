import { useForm, SubmitHandler, Resolver } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { CreateStreamerSchema, PlatformConfigSchema } from '../../api/schemas';
import { useQuery } from '@tanstack/react-query';
import { listPlatformConfigs, listTemplates, extractMetadata } from '@/server/functions';
import { Button } from '../ui/button';
import { Input } from '../ui/input';
import {
    Form,
    FormControl,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
    FormDescription,
} from '../ui/form';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '../ui/select';
import { Checkbox } from '../ui/checkbox';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../ui/card';
import { Separator } from '../ui/separator';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Loader2, Save, Undo2, ArrowRight, ArrowLeft, CheckCircle2, Link, User } from 'lucide-react';
import { useNavigate } from '@tanstack/react-router';
import { useState } from 'react';
import { Badge } from '../ui/badge';
import { Alert, AlertDescription, AlertTitle } from "../ui/alert"

type StreamerFormValues = z.infer<typeof CreateStreamerSchema>;
type PlatformConfig = z.infer<typeof PlatformConfigSchema>;

interface StreamerFormProps {
    defaultValues?: Partial<StreamerFormValues>;
    onSubmit: SubmitHandler<StreamerFormValues>;
    isSubmitting: boolean;
    title: React.ReactNode;
    description: React.ReactNode;
    submitLabel?: React.ReactNode;
}

export function StreamerForm({
    defaultValues,
    onSubmit,
    isSubmitting,
    title,
    description,
    submitLabel,
}: StreamerFormProps) {
    const navigate = useNavigate();
    const [stage, setStage] = useState<1 | 2>(1);
    const [detectingPlatform, setDetectingPlatform] = useState(false);
    const [detectedPlatform, setDetectedPlatform] = useState<string | null>(null);
    const [validPlatformConfigs, setValidPlatformConfigs] = useState<PlatformConfig[]>([]);

    // Fetch dependencies
    const { data: allPlatforms, isLoading: platformsLoading } = useQuery({
        queryKey: ['platforms'],
        queryFn: () => listPlatformConfigs(),
    });

    const { data: templates, isLoading: templatesLoading } = useQuery({
        queryKey: ['templates'],
        queryFn: () => listTemplates(),
    });

    const defaults: StreamerFormValues = {
        name: defaultValues?.name ?? '',
        url: defaultValues?.url ?? '',
        priority: defaultValues?.priority ?? 'NORMAL',
        enabled: defaultValues?.enabled ?? true,
        platform_config_id: defaultValues?.platform_config_id,
        template_id: defaultValues?.template_id,
    };

    const form = useForm<StreamerFormValues>({
        resolver: zodResolver(CreateStreamerSchema) as unknown as Resolver<StreamerFormValues>,
        defaultValues: defaults,
        mode: 'onChange', // Validate on change so we can disable Next button if needed
    });

    const handleNext = async () => {
        const url = form.getValues('url');

        // Manual validation for Stage 1 fields
        const urlValid = await form.trigger('url');
        const nameValid = await form.trigger('name');

        if (!urlValid || !nameValid) return;

        setDetectingPlatform(true);
        try {
            const metadata = await extractMetadata({ data: url });
            setDetectedPlatform(metadata.platform);
            setValidPlatformConfigs(metadata.valid_platform_configs);

            // If only one valid config and user hasn't selected one, select it
            if (metadata.valid_platform_configs.length === 1 && !form.getValues('platform_config_id')) {
                form.setValue('platform_config_id', metadata.valid_platform_configs[0].id);
            }

            // If channel ID detected and name is empty (though it shouldn't be valid if empty), prefill?
            // Name is required, so user must have entered it.

            setStage(2);
        } catch (error) {
            console.error("Failed to extract metadata:", error);
            // Even if extraction fails, let user proceed but show all platforms?
            // Or maybe just show an error toast.
            // For now, let's proceed with all platforms if extraction fails but show warning.
            setValidPlatformConfigs(allPlatforms || []);
            setStage(2);
        } finally {
            setDetectingPlatform(false);
        }
    };

    const availablePlatforms = validPlatformConfigs.length > 0 ? validPlatformConfigs : (allPlatforms || []);

    const isLoading = platformsLoading || templatesLoading;

    if (isLoading) {
        return (
            <div className="flex justify-center p-8">
                <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
            </div>
        );
    }

    return (
        <Card className="max-w-2xl mx-auto border-muted/40 shadow-sm overflow-hidden">
            <div className="absolute top-0 left-0 w-full h-1 bg-muted/20">
                <div
                    className="h-full bg-primary transition-all duration-500 ease-in-out"
                    style={{ width: stage === 1 ? '50%' : '100%' }}
                />
            </div>

            <CardHeader className="pb-4 border-b border-border/40 bg-muted/5">
                <div className="flex justify-between items-center">
                    <div>
                        <CardTitle className="text-xl">{title}</CardTitle>
                        <CardDescription>{description}</CardDescription>
                    </div>
                    <Badge variant="outline" className="ml-4">
                        <Trans>Step {stage} of 2</Trans>
                    </Badge>
                </div>
            </CardHeader>

            <CardContent className="p-5">
                <Form {...form}>
                    <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">

                        {/* Stage 1: Basic Info */}
                        <div className={stage === 1 ? "block space-y-4 animate-in fade-in slide-in-from-right-4 duration-300" : "hidden"}>
                            <div className="space-y-4">
                                <h3 className="text-sm font-medium text-muted-foreground uppercase tracking-wider mb-2">
                                    <Trans>Stream Details</Trans>
                                </h3>
                                <FormField
                                    control={form.control}
                                    name="url"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>URL</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <Link className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input
                                                        placeholder="https://twitch.tv/..."
                                                        {...field}
                                                        className="bg-background/50 font-mono text-sm pl-9"
                                                        autoFocus
                                                    />
                                                </div>
                                            </FormControl>
                                            <FormDescription>
                                                <Trans>The direct link to the channel or stream.</Trans>
                                            </FormDescription>
                                            <FormMessage />
                                        </FormItem>
                                    )}
                                />

                                <FormField
                                    control={form.control}
                                    name="name"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Name</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <User className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input
                                                        placeholder={t`e.g. My Favorite Streamer`}
                                                        {...field}
                                                        className="bg-background/50 pl-9"
                                                    />
                                                </div>
                                            </FormControl>
                                            <FormMessage />
                                        </FormItem>
                                    )}
                                />
                            </div>
                        </div>

                        {/* Stage 2: Configuration */}
                        <div className={stage === 2 ? "block space-y-6 animate-in fade-in slide-in-from-right-4 duration-300" : "hidden"}>

                            {/* Platform Detection Result */}
                            {detectedPlatform ? (
                                <Alert className="bg-primary/5 border-primary/20">
                                    <CheckCircle2 className="h-4 w-4 text-primary" />
                                    <AlertTitle className="text-primary font-medium">
                                        <Trans>Platform Detected: {detectedPlatform}</Trans>
                                    </AlertTitle>
                                    <AlertDescription className="text-muted-foreground text-xs">
                                        <Trans>Settings have been optimized for this platform.</Trans>
                                    </AlertDescription>
                                </Alert>
                            ) : null}

                            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                                <FormField
                                    control={form.control}
                                    name="platform_config_id"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Platform Configuration</Trans></FormLabel>
                                            <Select
                                                onValueChange={field.onChange}
                                                value={field.value || "none"}
                                            >
                                                <FormControl>
                                                    <SelectTrigger className="bg-background/50">
                                                        <SelectValue placeholder={t`Select config`} />
                                                    </SelectTrigger>
                                                </FormControl>
                                                <SelectContent>
                                                    <SelectItem value="none"><Trans>None (Default)</Trans></SelectItem>
                                                    {availablePlatforms.map((platform) => (
                                                        <SelectItem key={platform.id} value={platform.id}>
                                                            {platform.name}
                                                        </SelectItem>
                                                    ))}
                                                </SelectContent>
                                            </Select>
                                            <FormDescription>
                                                <Trans>Specific settings for {detectedPlatform || "the platform"}.</Trans>
                                            </FormDescription>
                                            <FormMessage />
                                        </FormItem>
                                    )}
                                />

                                <FormField
                                    control={form.control}
                                    name="template_id"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Template</Trans></FormLabel>
                                            <Select
                                                onValueChange={field.onChange}
                                                value={field.value || "none"}
                                            >
                                                <FormControl>
                                                    <SelectTrigger className="bg-background/50">
                                                        <SelectValue placeholder={t`Select template`} />
                                                    </SelectTrigger>
                                                </FormControl>
                                                <SelectContent>
                                                    <SelectItem value="none"><Trans>None (Default)</Trans></SelectItem>
                                                    {templates?.map((template) => (
                                                        <SelectItem key={template.id} value={template.id}>
                                                            {template.name}
                                                        </SelectItem>
                                                    ))}
                                                </SelectContent>
                                            </Select>
                                            <FormDescription>
                                                <Trans>Apply template settings.</Trans>
                                            </FormDescription>
                                            <FormMessage />
                                        </FormItem>
                                    )}
                                />
                            </div>

                            <Separator className="bg-border/50" />

                            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                                <FormField
                                    control={form.control}
                                    name="priority"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Priority</Trans></FormLabel>
                                            <Select onValueChange={field.onChange} value={field.value}>
                                                <FormControl>
                                                    <SelectTrigger className="bg-background/50">
                                                        <SelectValue placeholder={t`Select priority`} />
                                                    </SelectTrigger>
                                                </FormControl>
                                                <SelectContent>
                                                    <SelectItem value="HIGH"><Trans>High</Trans></SelectItem>
                                                    <SelectItem value="NORMAL"><Trans>Normal</Trans></SelectItem>
                                                    <SelectItem value="LOW"><Trans>Low</Trans></SelectItem>
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
                                        <FormItem className="flex flex-row items-start space-x-3 space-y-0 rounded-lg border border-border/50 p-4 bg-muted/20 mt-1">
                                            <FormControl>
                                                <Checkbox
                                                    checked={field.value}
                                                    onCheckedChange={field.onChange}
                                                />
                                            </FormControl>
                                            <div className="space-y-1 leading-none">
                                                <FormLabel className="font-semibold cursor-pointer">
                                                    <Trans>Enable Monitoring</Trans>
                                                </FormLabel>
                                                <FormDescription>
                                                    <Trans>Start checking this streamer immediately.</Trans>
                                                </FormDescription>
                                            </div>
                                        </FormItem>
                                    )}
                                />
                            </div>
                        </div>

                        {/* Footer Actions */}
                        <div className="flex justify-between pt-6 border-t border-border/40 mt-6">
                            {stage === 1 ? (
                                <>
                                    <Button variant="ghost" type="button" onClick={() => navigate({ to: '/dashboard' })}>
                                        <Undo2 className="mr-2 h-4 w-4" /> <Trans>Cancel</Trans>
                                    </Button>
                                    <Button
                                        type="button"
                                        onClick={handleNext}
                                        disabled={detectingPlatform}
                                        className="min-w-[120px]"
                                    >
                                        {detectingPlatform ? (
                                            <><Loader2 className="mr-2 h-4 w-4 animate-spin" /><Trans>Checking...</Trans></>
                                        ) : (
                                            <><Trans>Next</Trans> <ArrowRight className="ml-2 h-4 w-4" /></>
                                        )}
                                    </Button>
                                </>
                            ) : (
                                <>
                                    <Button variant="outline" type="button" onClick={() => setStage(1)}>
                                        <ArrowLeft className="mr-2 h-4 w-4" /> <Trans>Back</Trans>
                                    </Button>
                                    <Button type="submit" disabled={isSubmitting} className="min-w-[140px]">
                                        {isSubmitting ? (
                                            <><Loader2 className="mr-2 h-4 w-4 animate-spin" /> <Trans>Saving...</Trans></>
                                        ) : (
                                            <><Save className="mr-2 h-4 w-4" /> {submitLabel || <Trans>Save Streamer</Trans>}</>
                                        )}
                                    </Button>
                                </>
                            )}
                        </div>
                    </form>
                </Form>
            </CardContent>
        </Card>
    );
}
