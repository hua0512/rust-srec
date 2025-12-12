import { useState } from 'react';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { useQuery } from '@tanstack/react-query';
import { motion, Reorder, AnimatePresence } from 'motion/react';
import { ArrowLeft, Plus, GripVertical, X, Workflow, Save, Settings2, Sparkles, Layout } from 'lucide-react';
import { useNavigate } from '@tanstack/react-router';
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
    Form,
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from "@/components/ui/form";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Badge } from "@/components/ui/badge";
import { listJobPresets } from '@/server/functions/job';
import type { PipelinePreset } from '@/server/functions/pipeline';

const workflowSchema = z.object({
    name: z.string().min(1, 'Name is required'),
    description: z.string().optional(),
    steps: z.array(z.string()).min(1, 'At least one step is required'),
});

type WorkflowFormData = z.infer<typeof workflowSchema>;

interface WorkflowEditorProps {
    initialData?: PipelinePreset;
    title: React.ReactNode;
    onSubmit: (data: WorkflowFormData) => void;
    isUpdating?: boolean;
}

import { getStepColor, getStepIcon } from '@/components/pipeline/constants';

export function WorkflowEditor({ initialData, title, onSubmit, isUpdating }: WorkflowEditorProps) {
    const navigate = useNavigate();
    const [selectedPreset, setSelectedPreset] = useState<string>('');

    // Fetch available job presets
    const { data: presetsData } = useQuery({
        queryKey: ['job', 'presets'],
        queryFn: () => listJobPresets({ data: {} }),
    });

    const presets = presetsData?.presets || [];

    // Parse initial steps
    let initialSteps: string[] = [];
    if (initialData?.steps) {
        try {
            initialSteps = typeof initialData.steps === 'string'
                ? JSON.parse(initialData.steps)
                : initialData.steps;
        } catch { }
    }

    const form = useForm<WorkflowFormData>({
        resolver: zodResolver(workflowSchema),
        defaultValues: {
            name: initialData?.name || '',
            description: initialData?.description || '',
            steps: initialSteps,
        },
    });

    const steps = form.watch('steps');

    const handleAddStep = () => {
        if (selectedPreset) {
            const currentSteps = form.getValues('steps');
            form.setValue('steps', [...currentSteps, selectedPreset], { shouldDirty: true });
            setSelectedPreset('');
        }
    };

    const handleRemoveStep = (index: number) => {
        const currentSteps = form.getValues('steps');
        form.setValue('steps', currentSteps.filter((_, i) => i !== index), { shouldDirty: true });
    };

    const handleReorder = (newOrder: string[]) => {
        form.setValue('steps', newOrder, { shouldDirty: true });
    };

    const getPresetInfo = (name: string) => {
        return presets.find(p => p.name === name);
    };

    return (
        <div className="min-h-screen flex flex-col">
            {/* Header */}
            <div className="border-b border-border/40">
                <div className="max-w-[1600px] mx-auto px-4 md:px-8 py-3 flex items-center justify-between">
                    <div className="flex items-center gap-4">
                        <Button
                            variant="ghost"
                            size="icon"
                            onClick={() => navigate({ to: '/pipeline/workflows' })}
                            className="shrink-0 rounded-full hover:bg-muted/60"
                        >
                            <ArrowLeft className="h-5 w-5" />
                        </Button>
                        <div className="flex items-center gap-3">
                            <div className="p-2 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
                                <Workflow className="h-5 w-5 text-primary" />
                            </div>
                            <div>
                                <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
                                <p className="text-xs text-muted-foreground mr-6">
                                    <Trans>Design your automation pipeline step by step</Trans>
                                </p>
                            </div>
                        </div>
                    </div>
                    <div className="flex items-center gap-2">
                        <Button
                            onClick={form.handleSubmit(onSubmit)}
                            disabled={isUpdating || !form.formState.isDirty}
                            className="gap-2 shadow-lg shadow-primary/20"
                        >
                            <Save className="h-4 w-4" />
                            {isUpdating ? <Trans>Saving...</Trans> : <Trans>Save Workflow</Trans>}
                        </Button>
                    </div>
                </div>
            </div>

            <div className="flex-1 max-w-[1600px] mx-auto w-full px-4 md:px-8 py-8">
                <Form {...form}>
                    <form onSubmit={form.handleSubmit(onSubmit)} className="grid grid-cols-1 lg:grid-cols-12 gap-8 h-full">
                        {/* Left Column: Basic Info & Toolbox */}
                        <div className="lg:col-span-4 space-y-6">
                            <motion.div
                                initial={{ opacity: 0, x: -20 }}
                                animate={{ opacity: 1, x: 0 }}
                                className="group relative overflow-hidden rounded-2xl border border-border/40 bg-gradient-to-br from-background/50 to-background/20 backdrop-blur-xl transition-all hover:border-primary/20"
                            >
                                <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />
                                <div className="p-6 space-y-6">
                                    <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                                        <Settings2 className="h-4 w-4 text-primary" />
                                        <h3 className="font-medium tracking-tight"><Trans>Configuration</Trans></h3>
                                    </div>

                                    <FormField
                                        control={form.control}
                                        name="name"
                                        render={({ field }) => (
                                            <FormItem>
                                                <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold"><Trans>Name</Trans></FormLabel>
                                                <FormControl>
                                                    <Input
                                                        placeholder={t`e.g., Standard Processing`}
                                                        className="bg-muted/30 border-border/40 focus:bg-background/50 transition-colors"
                                                        {...field}
                                                    />
                                                </FormControl>
                                                <FormMessage />
                                            </FormItem>
                                        )}
                                    />

                                    <FormField
                                        control={form.control}
                                        name="description"
                                        render={({ field }) => (
                                            <FormItem>
                                                <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold"><Trans>Description</Trans></FormLabel>
                                                <FormControl>
                                                    <Textarea
                                                        placeholder={t`Describe what this workflow does...`}
                                                        className="resize-none bg-muted/30 border-border/40 focus:bg-background/50 transition-colors min-h-[120px]"
                                                        {...field}
                                                    />
                                                </FormControl>
                                                <FormMessage />
                                            </FormItem>
                                        )}
                                    />
                                </div>
                            </motion.div>

                            <motion.div
                                initial={{ opacity: 0, x: -20 }}
                                animate={{ opacity: 1, x: 0 }}
                                transition={{ delay: 0.1 }}
                                className="group relative overflow-hidden rounded-2xl border border-border/40 bg-gradient-to-br from-background/50 to-background/20 backdrop-blur-xl transition-all hover:border-primary/20"
                            >
                                <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />
                                <div className="p-6 space-y-6">
                                    <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                                        <Sparkles className="h-4 w-4 text-primary" />
                                        <h3 className="font-medium tracking-tight"><Trans>Add Steps</Trans></h3>
                                    </div>

                                    <div className="space-y-4">
                                        <div className="bg-muted/30 rounded-lg p-4 border border-border/40 text-sm text-muted-foreground">
                                            <Trans>Select a preset from the list below to add it to your workflow content pipeline.</Trans>
                                        </div>

                                        <div className="flex gap-2">
                                            <Select value={selectedPreset} onValueChange={setSelectedPreset}>
                                                <SelectTrigger className="flex-1 bg-muted/30 border-border/40 focus:ring-primary/20">
                                                    <SelectValue placeholder={t`Select a preset...`} />
                                                </SelectTrigger>
                                                <SelectContent>
                                                    {presets.map((preset) => {
                                                        const Icon = getStepIcon(preset.processor);
                                                        return (
                                                            <SelectItem key={preset.id} value={preset.name}>
                                                                <div className="flex items-center gap-2">
                                                                    <Icon className="h-4 w-4 text-muted-foreground" />
                                                                    <span>{preset.name}</span>
                                                                    {preset.category && (
                                                                        <Badge variant="secondary" className="text-[10px] ml-auto h-5">
                                                                            {preset.category}
                                                                        </Badge>
                                                                    )}
                                                                </div>
                                                            </SelectItem>
                                                        );
                                                    })}
                                                </SelectContent>
                                            </Select>
                                            <Button
                                                type="button"
                                                onClick={handleAddStep}
                                                disabled={!selectedPreset}
                                                size="icon"
                                                className="shrink-0"
                                            >
                                                <Plus className="h-4 w-4" />
                                            </Button>
                                        </div>
                                    </div>
                                </div>
                            </motion.div>
                        </div>

                        {/* Right Column: Steps Visualizer */}
                        <div className="lg:col-span-8 flex flex-col h-full">
                            <div className="flex items-center justify-between mb-4 px-1">
                                <div className="flex items-center gap-2">
                                    <div className="p-2 rounded-lg bg-primary/10">
                                        <Layout className="h-4 w-4 text-primary" />
                                    </div>
                                    <h3 className="font-semibold tracking-tight"><Trans>Pipeline Sequence</Trans></h3>
                                </div>
                                <Badge variant="outline" className="px-3 bg-background/50 backdrop-blur">
                                    {steps.length} <Trans>Steps</Trans>
                                </Badge>
                            </div>

                            <div className="flex-1 rounded-2xl border border-dashed border-border/60 bg-muted/5 min-h-[400px] p-6">
                                {steps.length > 0 ? (
                                    <Reorder.Group
                                        axis="y"
                                        values={steps}
                                        onReorder={handleReorder}
                                        className="space-y-3"
                                    >
                                        <AnimatePresence mode='popLayout'>
                                            {steps.map((step, index) => {
                                                const presetInfo = getPresetInfo(step);
                                                const Icon = presetInfo ? getStepIcon(presetInfo.processor) : Workflow;
                                                const colorClass = presetInfo
                                                    ? getStepColor(presetInfo.processor, presetInfo.category || undefined)
                                                    : "from-muted/20 to-muted/10 text-muted-foreground border-border";

                                                return (
                                                    <Reorder.Item
                                                        key={`${step}-${index}`}
                                                        value={step}
                                                        className="relative"
                                                    >
                                                        <motion.div
                                                            layout
                                                            initial={{ opacity: 0, y: 10, scale: 0.98 }}
                                                            animate={{ opacity: 1, y: 0, scale: 1 }}
                                                            exit={{ opacity: 0, scale: 0.9, transition: { duration: 0.2 } }}
                                                            whileHover={{ scale: 1.01 }}
                                                            whileTap={{ scale: 0.99 }}
                                                            className={`
                                                                group flex items-center gap-4 p-4 rounded-xl border bg-gradient-to-r ${colorClass} 
                                                                shadow-sm hover:shadow-md transition-all cursor-grab active:cursor-grabbing
                                                            `}
                                                        >
                                                            <div className="flex items-center justify-center h-8 w-8 rounded-lg bg-background/40 backdrop-blur text-inherit shrink-0 border border-current border-opacity-20 shadow-sm">
                                                                <span className="text-sm font-bold font-mono opacity-80">{index + 1}</span>
                                                            </div>

                                                            <div className="h-10 w-10 rounded-xl bg-background/40 backdrop-blur flex items-center justify-center shrink-0 border border-current border-opacity-20 shadow-sm">
                                                                <Icon className="h-5 w-5 opacity-90" />
                                                            </div>

                                                            <div className="flex-1 min-w-0 flex flex-col justify-center">
                                                                <div className="flex items-center gap-2">
                                                                    <span className="font-semibold tracking-tight truncate text-foreground/90">{step}</span>
                                                                    {presetInfo?.category && (
                                                                        <Badge variant="outline" className="text-[10px] uppercase h-4 px-1.5 bg-background/30 backdrop-blur border-current border-opacity-20 text-inherit">
                                                                            {presetInfo.category}
                                                                        </Badge>
                                                                    )}
                                                                </div>
                                                                {presetInfo?.description && (
                                                                    <span className="text-xs text-muted-foreground/80 truncate opacity-90 max-w-[80%]">
                                                                        {presetInfo.description}
                                                                    </span>
                                                                )}
                                                            </div>

                                                            <GripVertical className="h-5 w-5 opacity-20 group-hover:opacity-50 transition-opacity mr-2" />

                                                            <Button
                                                                type="button"
                                                                variant="ghost"
                                                                size="icon"
                                                                className="h-9 w-9 shrink-0 opacity-0 group-hover:opacity-100 transition-all hover:bg-destructive/10 hover:text-destructive rounded-xl"
                                                                onClick={() => handleRemoveStep(index)}
                                                            >
                                                                <X className="h-4 w-4" />
                                                            </Button>
                                                        </motion.div>

                                                        {/* Connector Line */}
                                                        {index < steps.length - 1 && (
                                                            <div className="absolute left-8 top-full h-3 w-px bg-border/60 -ml-px z-0" />
                                                        )}
                                                    </Reorder.Item>
                                                );
                                            })}
                                        </AnimatePresence>
                                    </Reorder.Group>
                                ) : (
                                    <div className="h-full min-h-[350px] flex flex-col items-center justify-center text-center space-y-4">
                                        <div className="p-6 rounded-full bg-muted/30 border border-dashed border-border mb-2 animate-pulse">
                                            <Workflow className="h-10 w-10 text-muted-foreground/40" />
                                        </div>
                                        <div className="space-y-1 max-w-sm">
                                            <h3 className="font-medium text-foreground"><Trans>No Steps Added</Trans></h3>
                                            <p className="text-sm text-muted-foreground">
                                                <Trans>Your workflow pipeline is empty. Add steps from the configuration panel on the left to build your automation.</Trans>
                                            </p>
                                        </div>
                                    </div>
                                )}
                            </div>
                        </div>
                    </form>
                </Form>
            </div>
        </div>
    );
}
