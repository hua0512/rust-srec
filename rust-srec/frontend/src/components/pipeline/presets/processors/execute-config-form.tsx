import { useLingui } from "@lingui/react";
import { Trans, t } from "@lingui/macro";
import { FormField, FormItem, FormLabel, FormControl, FormMessage, FormDescription } from "@/components/ui/form";
import { Textarea } from "@/components/ui/textarea";
import { Input } from "@/components/ui/input";
import { ProcessorConfigFormProps } from "./common-props";
import { ExecuteConfigSchema } from "../processor-schemas";
import { z } from "zod";
import { motion } from "framer-motion";
import { Badge } from "@/components/ui/badge";
import { Terminal, FolderSearch } from "lucide-react";

type ExecuteConfig = z.infer<typeof ExecuteConfigSchema>;

export function ExecuteConfigForm({ control, pathPrefix }: ProcessorConfigFormProps<ExecuteConfig>) {
    const { i18n } = useLingui();
    const prefix = pathPrefix ? `${pathPrefix}.` : "";

    const containerVariants = {
        hidden: { opacity: 0, y: 20 },
        visible: { opacity: 1, y: 0, transition: { duration: 0.3 } }
    };

    return (
        <motion.div
            variants={containerVariants}
            initial="hidden"
            animate="visible"
            className="w-full"
        >
            <div className="space-y-6">
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                    <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                        <Terminal className="w-4 h-4 text-green-500" />
                        <h3 className="font-semibold text-sm mr-auto"><Trans>Script</Trans></h3>
                    </div>

                    <div className="grid grid-cols-1 gap-6">
                        <FormField
                            control={control}
                            name={`${prefix}command` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1"><Trans>Command</Trans></FormLabel>
                                    <FormControl>
                                        <Textarea
                                            placeholder={t(i18n)`e.g. echo {input} > {output}`}
                                            className="font-mono bg-background/50 border-border/50 focus:bg-background rounded-lg text-sm"
                                            rows={5}
                                            {...field}
                                        />
                                    </FormControl>
                                    <FormDescription className="mt-2 text-sm max-w-full">
                                        <div className="p-3 border border-border/40 rounded-lg bg-muted/20">
                                            <div className="mb-2 font-semibold text-[10px] uppercase tracking-wide opacity-70"><Trans>Available Variables</Trans></div>
                                            <div className="flex flex-wrap gap-2">
                                                <Badge variant="outline" className="font-mono text-[10px] bg-background/50 cursor-help border-border/50" title="Input file path">{"{input}"}</Badge>
                                                <Badge variant="outline" className="font-mono text-[10px] bg-background/50 cursor-help border-border/50" title="Output file path">{"{output}"}</Badge>
                                                <Badge variant="outline" className="font-mono text-[10px] bg-background/50 cursor-help border-border/50" title="Input file directory">{"{input_dir}"}</Badge>
                                                <Badge variant="outline" className="font-mono text-[10px] bg-background/50 cursor-help border-border/50" title="Streamer ID">{"{streamer_id}"}</Badge>
                                                <Badge variant="outline" className="font-mono text-[10px] bg-background/50 cursor-help border-border/50" title="Session ID">{"{session_id}"}</Badge>
                                            </div>
                                        </div>
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </div>
                </div>

                {/* Output Scanning Section */}
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                    <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                        <FolderSearch className="w-4 h-4 text-blue-500" />
                        <h3 className="font-semibold text-sm mr-auto"><Trans>Output Detection</Trans></h3>
                        <Badge variant="outline" className="text-[10px]"><Trans>Optional</Trans></Badge>
                    </div>

                    <FormDescription className="text-xs text-muted-foreground">
                        <Trans>
                            Scan a directory for new files created by the command. Detected files will be passed to the next pipeline step.
                        </Trans>
                    </FormDescription>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                        <FormField
                            control={control}
                            name={`${prefix}scan_output_dir` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1"><Trans>Scan Directory</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            placeholder={t(i18n)`e.g. /output/processed/`}
                                            className="font-mono bg-background/50 border-border/50 focus:bg-background rounded-lg text-sm"
                                            {...field}
                                            value={field.value ?? ""}
                                        />
                                    </FormControl>
                                    <FormDescription className="text-xs">
                                        <Trans>Directory to scan for new files after command execution</Trans>
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <FormField
                            control={control}
                            name={`${prefix}scan_extension` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1"><Trans>File Extension Filter</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            placeholder={t(i18n)`e.g. mp4`}
                                            className="font-mono bg-background/50 border-border/50 focus:bg-background rounded-lg text-sm"
                                            {...field}
                                            value={field.value ?? ""}
                                        />
                                    </FormControl>
                                    <FormDescription className="text-xs">
                                        <Trans>Only include files with this extension (without dot)</Trans>
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </div>
                </div>
            </div>
        </motion.div>
    );
}
