import { UseFormReturn } from "react-hook-form";
import {
    FormControl,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Link } from "@tanstack/react-router";
import { ArrowLeft, Film, Image as ImageIcon, Music, FileArchive, Copy, Trash, Tags, Upload, Terminal, Settings2 } from "lucide-react";
import { Trans } from "@lingui/react/macro";
import { motion } from "framer-motion";

interface PresetMetaFormProps {
    form: UseFormReturn<any>;
    initialData?: any;
    title: React.ReactNode;
}

const PROCESSOR_OPTIONS = [
    { id: "remux", label: <Trans>Remux / Transcode</Trans>, icon: Film },
    { id: "thumbnail", label: <Trans>Thumbnail</Trans>, icon: ImageIcon },
    { id: "audio_extract", label: <Trans>Audio Extract</Trans>, icon: Music },
    { id: "compression", label: <Trans>Compression</Trans>, icon: FileArchive },
    { id: "copy_move", label: <Trans>Copy / Move</Trans>, icon: Copy },
    { id: "delete", label: <Trans>Delete</Trans>, icon: Trash },
    { id: "metadata", label: <Trans>Metadata</Trans>, icon: Tags },
    { id: "rclone", label: <Trans>Rclone (Upload / Move / Sync)</Trans>, icon: Upload },
    { id: "execute", label: <Trans>Execute Command</Trans>, icon: Terminal },
];

export function PresetMetaForm({ form, initialData, title }: PresetMetaFormProps) {
    const currentProcessor = form.watch("processor");
    // Find selected option to get label and icon safely
    const selectedOption = PROCESSOR_OPTIONS.find(opt => opt.id === currentProcessor);
    const CurrentIcon = selectedOption?.icon || Settings2;

    const handleProcessorChange = (value: string) => {
        form.setValue("processor", value);
    };

    return (
        <motion.div
            initial={{ opacity: 0, x: -20 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ duration: 0.4 }}
        >
            <Card className="border-border/40 shadow-sm bg-card/80 backdrop-blur-sm sticky top-6">
                <CardHeader className="pb-6 border-b border-border/40 bg-muted/10">
                    <div className="flex items-center gap-4">
                        <Button variant="ghost" size="icon" className="h-9 w-9 -ml-2 text-muted-foreground/70 hover:text-foreground hover:bg-background/50 rounded-full" asChild>
                            <Link to="/pipeline/presets">
                                <ArrowLeft className="h-5 w-5" />
                            </Link>
                        </Button>
                        <div className="flex flex-col gap-0.5">
                            <CardTitle className="text-lg font-semibold tracking-tight"><Trans>Preset Details</Trans></CardTitle>
                            <CardDescription className="text-xs font-normal text-muted-foreground/80">{title}</CardDescription>
                        </div>
                        <div className="ml-auto p-2 rounded-xl bg-background/50 border border-border/50 shadow-sm text-primary">
                            <CurrentIcon className="w-5 h-5" />
                        </div>
                    </div>
                </CardHeader>

                <CardContent className="space-y-6 p-6">
                    <FormField
                        control={form.control}
                        name="processor"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-medium ml-1"><Trans>Processor</Trans></FormLabel>
                                <Select onValueChange={handleProcessorChange} defaultValue={field.value} disabled={!!initialData}>
                                    <FormControl>
                                        <SelectTrigger className="h-12 bg-muted/30 border-muted-foreground/20 focus:ring-primary/20 transition-all font-medium">
                                            <div className="flex items-center gap-3">
                                                <div className="p-1 rounded bg-primary/10 text-primary">
                                                    <CurrentIcon className="w-4 h-4" />
                                                </div>
                                                {selectedOption ? (
                                                    <span className="text-sm">{selectedOption.label}</span>
                                                ) : (
                                                    <SelectValue placeholder="Select type" />
                                                )}
                                            </div>
                                        </SelectTrigger>
                                    </FormControl>
                                    <SelectContent>
                                        {PROCESSOR_OPTIONS.map((option) => (
                                            <SelectItem key={option.id} value={option.id}>
                                                <div className="flex items-center gap-2">
                                                    <option.icon className="w-4 h-4 text-muted-foreground" />
                                                    <span>{option.label}</span>
                                                </div>
                                            </SelectItem>
                                        ))}
                                    </SelectContent>
                                </Select>
                                <FormMessage />
                            </FormItem>
                        )}
                    />

                    <div className="space-y-4">
                        <FormField
                            control={form.control}
                            name="id"
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-medium ml-1"><Trans>ID</Trans></FormLabel>
                                    <FormControl>
                                        <div className="relative">
                                            <Input
                                                {...field}
                                                placeholder="e.g. remux-h264"
                                                disabled={!!initialData}
                                                className="h-11 pl-9 bg-muted/30 border-muted-foreground/20 focus:bg-background transition-all font-mono text-sm"
                                            />
                                            <div className="absolute left-3 top-3.5 text-muted-foreground/50 text-xs">#</div>
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
                                    <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-medium ml-1"><Trans>Name</Trans></FormLabel>
                                    <FormControl>
                                        <Input
                                            {...field}
                                            placeholder="e.g. Remux to H.264"
                                            className="h-11 bg-muted/30 border-muted-foreground/20 focus:bg-background transition-all"
                                        />
                                    </FormControl>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </div>
                </CardContent>
            </Card>
        </motion.div>
    );
}
