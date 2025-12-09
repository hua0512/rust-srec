import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { t } from "@lingui/core/macro";
import { Trans } from "@lingui/react/macro";
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle,
    DialogTrigger,
} from '@/components/ui/dialog';
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
import { Button } from '@/components/ui/button';
import { Tag, Cpu } from 'lucide-react';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '@/components/ui/select';
import { toast } from 'sonner';
import {
    EngineConfigSchema,
    CreateEngineRequestSchema,
    UpdateEngineRequestSchema,
} from '@/api/schemas';
import { engineApi } from '@/api/endpoints';
import { useState, useEffect } from 'react';

import { FfmpegForm } from './forms/ffmpeg-form';
import { StreamlinkForm } from './forms/streamlink-form';
import { MesioForm } from './forms/mesio-form';

// Define specific config schemas for validation and defaults
const FfmpegConfigSchema = z.object({
    binary_path: z.string().default('ffmpeg'),
    input_args: z.array(z.string()).default([]),
    output_args: z.array(z.string()).default([]),
    timeout_secs: z.coerce.number().default(30),
    user_agent: z.string().optional(),
});

const StreamlinkConfigSchema = z.object({
    binary_path: z.string().default('streamlink'),
    quality: z.string().default('best'),
    extra_args: z.array(z.string()).default([]),
});

const MesioConfigSchema = z.object({
    buffer_size: z.coerce.number().default(8388608),
    fix_flv: z.boolean().default(true),
    fix_hls: z.boolean().default(true),
});

interface EditEngineDialogProps {
    engine?: z.infer<typeof EngineConfigSchema>;
    trigger?: React.ReactNode;
    open?: boolean;
    onOpenChange?: (open: boolean) => void;
}

export function EditEngineDialog({
    engine,
    trigger,
    open: controlledOpen,
    onOpenChange: setControlledOpen,
}: EditEngineDialogProps) {
    const [open, setOpen] = useState(false);
    const isEdit = !!engine;
    const queryClient = useQueryClient();

    const form = useForm<z.infer<typeof CreateEngineRequestSchema>>({
        resolver: zodResolver(CreateEngineRequestSchema),
        defaultValues: {
            name: '',
            engine_type: 'FFMPEG',
            config: FfmpegConfigSchema.parse({}),
        },
    });

    useEffect(() => {
        if (engine) {
            let parsedConfig = {};
            try {
                parsedConfig = JSON.parse(engine.config);
            } catch (e) {
                console.error("Failed to parse engine config JSON", e);
            }
            form.reset({
                name: engine.name,
                engine_type: engine.engine_type,
                config: parsedConfig,
            });
        } else {
            form.reset({
                name: '',
                engine_type: 'FFMPEG',
                config: FfmpegConfigSchema.parse({}),
            });
        }
    }, [engine, form, open]);

    const engineType = form.watch('engine_type');

    const createMutation = useMutation({
        mutationFn: engineApi.create,
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['engines'] });
            toast.success(t`Engine configuration created`);
            setOpen(false);
            setControlledOpen?.(false);
        },
        onError: (error: Error) => {
            toast.error(t`Failed to create engine: ${error.message}`);
        },
    });

    const updateMutation = useMutation({
        mutationFn: (data: z.infer<typeof UpdateEngineRequestSchema>) =>
            engineApi.update(engine!.id, data),
        onSuccess: () => {
            queryClient.invalidateQueries({ queryKey: ['engines'] });
            toast.success(t`Engine configuration updated`);
            setOpen(false);
            setControlledOpen?.(false);
        },
        onError: (error: Error) => {
            toast.error(t`Failed to update engine: ${error.message}`);
        },
    });

    const onSubmit = (data: z.infer<typeof CreateEngineRequestSchema>) => {
        if (isEdit) {
            updateMutation.mutate(data);
        } else {
            createMutation.mutate(data);
        }
    };

    const handleOpenChange = (newOpen: boolean) => {
        setOpen(newOpen);
        setControlledOpen?.(newOpen);
        if (!newOpen) {
            form.reset();
        }
    };

    return (
        <Dialog open={controlledOpen ?? open} onOpenChange={handleOpenChange}>
            {trigger && <DialogTrigger asChild>{trigger}</DialogTrigger>}
            <DialogContent className="sm:max-w-[700px] max-h-[85vh] overflow-y-auto w-full">
                <DialogHeader className="mb-4">
                    <DialogTitle className="text-2xl">{isEdit ? t`Edit Engine` : t`New Engine`}</DialogTitle>
                    <DialogDescription>
                        <Trans>Configure the downloader engine properties.</Trans>
                    </DialogDescription>
                </DialogHeader>

                <Form {...form}>
                    <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-6">
                        <div className="grid gap-4 md:grid-cols-2">
                            <FormField
                                control={form.control}
                                name="name"
                                render={({ field }) => (
                                    <FormItem>
                                        <FormLabel className="flex items-center gap-2">
                                            <Tag className="w-4 h-4 text-primary" />
                                            <Trans>Name</Trans>
                                        </FormLabel>
                                        <FormControl>
                                            <Input placeholder={t`e.g. Production FFmpeg`} {...field} />
                                        </FormControl>
                                        <FormDescription>
                                            <Trans>Unique identifier for this config.</Trans>
                                        </FormDescription>
                                        <FormMessage />
                                    </FormItem>
                                )}
                            />

                            <FormField
                                control={form.control}
                                name="engine_type"
                                render={({ field }) => (
                                    <FormItem>
                                        <FormLabel className="flex items-center gap-2">
                                            <Cpu className="w-4 h-4 text-primary" />
                                            <Trans>Engine Type</Trans>
                                        </FormLabel>
                                        <Select
                                            onValueChange={(value) => {
                                                field.onChange(value);
                                                // Reset config to defaults of new type
                                                if (value === 'FFMPEG') form.setValue('config', FfmpegConfigSchema.parse({}));
                                                if (value === 'STREAMLINK') form.setValue('config', StreamlinkConfigSchema.parse({}));
                                                if (value === 'MESIO') form.setValue('config', MesioConfigSchema.parse({}));
                                            }}
                                            defaultValue={field.value}
                                        >
                                            <FormControl>
                                                <SelectTrigger className="w-full">
                                                    <SelectValue placeholder={t`Select type`} />
                                                </SelectTrigger>
                                            </FormControl>
                                            <SelectContent>
                                                <SelectItem value="FFMPEG">FFmpeg</SelectItem>
                                                <SelectItem value="STREAMLINK">Streamlink</SelectItem>
                                                <SelectItem value="MESIO">Mesio</SelectItem>
                                            </SelectContent>
                                        </Select>
                                        <FormDescription>
                                            <Trans>Protocol handler implementation.</Trans>
                                        </FormDescription>
                                        <FormMessage />
                                    </FormItem>
                                )}
                            />
                        </div>

                        {/* Dynamic Form Section */}
                        <div className="border-t pt-6">
                            {engineType === 'FFMPEG' && <FfmpegForm control={form.control} />}
                            {engineType === 'STREAMLINK' && <StreamlinkForm control={form.control} />}
                            {engineType === 'MESIO' && <MesioForm control={form.control} />}
                        </div>

                        <DialogFooter className="sticky bottom-0 bg-background pt-4 border-t mt-6">
                            <Button type="submit" disabled={createMutation.isPending || updateMutation.isPending} size="lg" className="w-full sm:w-auto">
                                {isEdit ? t`Save Changes` : t`Create Engine`}
                            </Button>
                        </DialogFooter>
                    </form>
                </Form>
            </DialogContent>
        </Dialog>
    );
}
