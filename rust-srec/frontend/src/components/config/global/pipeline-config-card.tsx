
import { useState } from "react";
import { Control } from "react-hook-form";
import { useQuery } from "@tanstack/react-query";
import { motion, Reorder, AnimatePresence } from "motion/react";
import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
} from "@/components/ui/card";
import {
    FormDescription,
    FormField,
    FormItem,
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
import { Button } from "@/components/ui/button";
import { Workflow, Plus, X, GripVertical } from "lucide-react";
import { Trans } from "@lingui/react/macro";
import { listJobPresets } from "@/server/functions/job";
import { getStepColor, getStepIcon } from "@/components/pipeline/constants";
import { t } from "@lingui/core/macro";

interface PipelineConfigCardProps {
    control: Control<any>;
}

export function PipelineConfigCard({ control }: PipelineConfigCardProps) {
    const [selectedPreset, setSelectedPreset] = useState<string>('');

    // Fetch available job presets
    const { data: presetsData } = useQuery({
        queryKey: ['job', 'presets'],
        queryFn: () => listJobPresets({ data: {} }),
    });
    const presets = presetsData?.presets || [];

    const getPresetInfo = (name: string) => presets.find(p => p.name === name);

    return (
        <Card className="h-full hover:shadow-md transition-all duration-300 border-muted/60">
            <CardHeader>
                <CardTitle className="flex items-center gap-3 text-xl">
                    <div className="p-2.5 bg-orange-500/10 text-orange-500 rounded-lg">
                        <Workflow className="w-5 h-5" />
                    </div>
                    <Trans>Pipeline Configuration</Trans>
                </CardTitle>
                <CardDescription className="pl-[3.25rem]">
                    <Trans>Default pipeline flow. Configure the sequence of processors for new jobs.</Trans>
                </CardDescription>
            </CardHeader>
            <CardContent className="space-y-6">
                <FormField
                    control={control}
                    name="pipeline"
                    render={({ field }) => {
                        // Parse JSON string to array safely
                        let steps: string[] = [];
                        try {
                            if (field.value) {
                                steps = JSON.parse(field.value);
                                if (!Array.isArray(steps)) steps = [];
                            }
                        } catch (e) {
                            // If invalid JSON, showing empty steps or handling error might be needed.
                            // For now, assume it might be a raw string if legacy, but we enforced JSON array mostly.
                        }

                        const updateSteps = (newSteps: string[]) => {
                            field.onChange(JSON.stringify(newSteps));
                        };

                        const handleAddStep = () => {
                            if (selectedPreset) {
                                updateSteps([...steps, selectedPreset]);
                                setSelectedPreset('');
                            }
                        };

                        const handleRemoveStep = (index: number) => {
                            updateSteps(steps.filter((_, i) => i !== index));
                        };

                        const handleReorder = (newOrder: string[]) => {
                            updateSteps(newOrder);
                        };

                        return (
                            <FormItem>
                                <div className="space-y-4">
                                    {/* Add Step Control */}
                                    <div className="flex gap-2">
                                        <Select value={selectedPreset} onValueChange={setSelectedPreset}>
                                            <SelectTrigger className="flex-1 bg-background/40 border-border/40 focus:ring-primary/20 backdrop-blur-sm">
                                                <SelectValue placeholder={t`Add a step to pipeline...`} />
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
                                            className="shrink-0 bg-primary/90 hover:bg-primary shadow-sm"
                                        >
                                            <Plus className="h-4 w-4" />
                                        </Button>
                                    </div>

                                    {/* Visual List */}
                                    <div className="rounded-xl border border-dashed border-border/60 bg-muted/5 min-h-[120px] p-4">
                                        {steps.length > 0 ? (
                                            <Reorder.Group
                                                axis="y"
                                                values={steps}
                                                onReorder={handleReorder}
                                                className="space-y-2"
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
                                                                    initial={{ opacity: 0, y: 5, scale: 0.98 }}
                                                                    animate={{ opacity: 1, y: 0, scale: 1 }}
                                                                    exit={{ opacity: 0, scale: 0.95 }}
                                                                    className={`
                                                                        group flex items-center gap-3 p-3 rounded-lg border bg-gradient-to-r ${colorClass} 
                                                                        shadow-sm hover:shadow transition-all cursor-grab active:cursor-grabbing
                                                                    `}
                                                                >
                                                                    <div className="flex items-center justify-center h-6 w-6 rounded bg-background/40 backdrop-blur text-inherit shrink-0 border border-current border-opacity-20">
                                                                        <span className="text-xs font-bold font-mono opacity-80">{index + 1}</span>
                                                                    </div>

                                                                    <div className="flex items-center justify-center shrink-0 opacity-80">
                                                                        <Icon className="h-4 w-4" />
                                                                    </div>

                                                                    <span className="font-medium text-sm tracking-tight truncate text-foreground/90 flex-1">{step}</span>

                                                                    <GripVertical className="h-4 w-4 opacity-0 group-hover:opacity-40 transition-opacity" />

                                                                    <Button
                                                                        type="button"
                                                                        variant="ghost"
                                                                        size="icon"
                                                                        className="h-7 w-7 shrink-0 opacity-0 group-hover:opacity-100 transition-all hover:bg-destructive/10 hover:text-destructive rounded-lg -mr-1"
                                                                        onClick={() => handleRemoveStep(index)}
                                                                    >
                                                                        <X className="h-3 w-3" />
                                                                    </Button>
                                                                </motion.div>
                                                            </Reorder.Item>
                                                        );
                                                    })}
                                                </AnimatePresence>
                                            </Reorder.Group>
                                        ) : (
                                            <div className="h-full py-8 flex flex-col items-center justify-center text-center space-y-2 opacity-60">
                                                <Workflow className="h-8 w-8 text-muted-foreground/40" />
                                                <p className="text-sm text-muted-foreground"><Trans>Pipeline is empty</Trans></p>
                                            </div>
                                        )}
                                    </div>
                                    <FormDescription className="text-xs">
                                        <Trans>Drag to reorder steps.</Trans>
                                    </FormDescription>
                                </div>
                                <FormMessage />
                            </FormItem>
                        );
                    }}
                />
            </CardContent>
        </Card>
    );
}
