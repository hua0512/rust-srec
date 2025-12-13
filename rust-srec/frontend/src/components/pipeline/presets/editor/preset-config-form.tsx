import { Button } from "@/components/ui/button";
import { Wand2 } from "lucide-react";
import { Trans } from "@lingui/react/macro";
import { PRESET_TEMPLATES } from "../preset-templates";
import { ProcessorConfigManager } from "../processors/processor-config-manager";
import { motion } from "motion/react";
import { UseFormReturn } from "react-hook-form";
import { toast } from "sonner";
import { t } from "@lingui/core/macro";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

interface PresetConfigFormProps {
    form: UseFormReturn<any>;
    currentProcessor: string;
}

export function PresetConfigForm({ form, currentProcessor }: PresetConfigFormProps) {
    const loadTemplate = (templateKey: keyof typeof PRESET_TEMPLATES) => {
        const template = PRESET_TEMPLATES[templateKey];
        form.setValue("config", template.value);
        toast.info(t`Loaded template: ${template.label}`);
    };

    return (
        <motion.div
            initial={{ opacity: 0, y: 20 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.4, delay: 0.1 }}
        >
            <Card className="border-border/40 shadow-sm bg-card/80 backdrop-blur-sm">
                <CardHeader className="pb-6 border-b border-border/40 bg-muted/10">
                    <div className="flex justify-between items-center">
                        <div className="flex flex-col gap-0.5">
                            <CardTitle className="text-lg font-semibold tracking-tight"><Trans>Configuration</Trans></CardTitle>
                            <CardDescription className="text-xs font-normal text-muted-foreground/80"><Trans>Detailed processor settings</Trans></CardDescription>
                        </div>
                        <div className="flex gap-2">
                            {currentProcessor === 'remux' && (
                                <>
                                    <Button type="button" variant="secondary" size="sm" onClick={() => loadTemplate('remux')} className="h-8 text-xs font-medium bg-secondary/50 hover:bg-secondary text-secondary-foreground gap-1.5 rounded-lg transition-colors">
                                        <Wand2 className="w-3.5 h-3.5" /> <Trans>Copy</Trans>
                                    </Button>
                                    <Button type="button" variant="secondary" size="sm" onClick={() => loadTemplate('transcode_h264')} className="h-8 text-xs font-medium bg-secondary/50 hover:bg-secondary text-secondary-foreground gap-1.5 rounded-lg transition-colors">
                                        <Wand2 className="w-3.5 h-3.5" /> <Trans>H.264</Trans>
                                    </Button>
                                </>
                            )}
                            {(currentProcessor === 'upload' || currentProcessor === 'rclone') && (
                                <Button type="button" variant="secondary" size="sm" onClick={() => loadTemplate('rclone')} className="h-8 text-xs font-medium bg-secondary/50 hover:bg-secondary text-secondary-foreground gap-1.5 rounded-lg transition-colors">
                                    <Wand2 className="w-3.5 h-3.5" /> <Trans>Default</Trans>
                                </Button>
                            )}
                            {currentProcessor === 'thumbnail' && (
                                <Button type="button" variant="secondary" size="sm" onClick={() => loadTemplate('thumbnail')} className="h-8 text-xs font-medium bg-secondary/50 hover:bg-secondary text-secondary-foreground gap-1.5 rounded-lg transition-colors">
                                    <Wand2 className="w-3.5 h-3.5" /> <Trans>Default</Trans>
                                </Button>
                            )}
                        </div>
                    </div>
                </CardHeader>
                <CardContent className="p-6 md:p-8">
                    <ProcessorConfigManager
                        processorType={currentProcessor}
                        control={form.control}
                        register={form.register}
                        pathPrefix="config"
                    />
                </CardContent>
            </Card>
        </motion.div>
    );
}
