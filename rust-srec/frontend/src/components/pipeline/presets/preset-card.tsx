import { JobPresetSchema } from "@/api/schemas";
import { Button } from "@/components/ui/button";
import {
    Card,
    CardContent,
    CardDescription,
    CardFooter,
    CardHeader,
    CardTitle,
} from "@/components/ui/card";
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
    AlertDialog,
    AlertDialogAction,
    AlertDialogCancel,
    AlertDialogContent,
    AlertDialogDescription,
    AlertDialogFooter,
    AlertDialogHeader,
    AlertDialogTitle,
    AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Edit, MoreHorizontal, Trash, FileVideo, Upload, Terminal, Copy, Scissors, Archive, Tag, Image as ImageIcon, CopyPlus, Cloud } from "lucide-react";
import { z } from "zod";
import { Trans } from "@lingui/react/macro";


interface PresetCardProps {
    preset: z.infer<typeof JobPresetSchema>;
    onEdit: (preset: z.infer<typeof JobPresetSchema>) => void;
    onDelete: (id: string) => void;
    onClone?: (preset: z.infer<typeof JobPresetSchema>) => void;
}

const PROCESSOR_ICONS: Record<string, React.ElementType> = {
    remux: FileVideo,
    thumbnail: ImageIcon,
    upload: Upload,
    rclone: Cloud,
    execute: Terminal,
    copy_move: Copy,
    audio_extract: Scissors,
    compression: Archive,
    delete: Trash,
    metadata: Tag,
};

const PROCESSOR_COLORS: Record<string, string> = {
    remux: "bg-blue-500/10 text-blue-500 border-blue-500/20",
    thumbnail: "bg-purple-500/10 text-purple-500 border-purple-500/20",
    upload: "bg-green-500/10 text-green-500 border-green-500/20",
    rclone: "bg-emerald-500/10 text-emerald-500 border-emerald-500/20",
    execute: "bg-gray-500/10 text-gray-500 border-gray-500/20",
    audio_extract: "bg-pink-500/10 text-pink-500 border-pink-500/20",
    compression: "bg-orange-500/10 text-orange-500 border-orange-500/20",
    delete: "bg-red-500/10 text-red-500 border-red-500/20",
    metadata: "bg-cyan-500/10 text-cyan-500 border-cyan-500/20",
    copy_move: "bg-amber-500/10 text-amber-500 border-amber-500/20",
};

const CATEGORY_LABELS: Record<string, string> = {
    remux: "Remux",
    compression: "Compression",
    thumbnail: "Thumbnail",
    audio: "Audio",
    archive: "Archive",
    upload: "Upload",
    cleanup: "Cleanup",
    file_ops: "File Ops",
    custom: "Custom",
    metadata: "Metadata",
};

export function PresetCard({ preset, onEdit, onDelete, onClone }: PresetCardProps) {
    const Icon = PROCESSOR_ICONS[preset.processor] || FileVideo;
    const colorClass = PROCESSOR_COLORS[preset.processor] || "bg-primary/10 text-primary border-primary/20";
    const categoryLabel = preset.category ? CATEGORY_LABELS[preset.category] || preset.category : null;

    let configObj: any = {};
    try {
        configObj = typeof preset.config === 'string' ? JSON.parse(preset.config) : preset.config;
    } catch { }

    const configKeys = Object.keys(configObj).filter(k => k !== 'overwrite' && k !== 'create_dirs');

    return (
        <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
            <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

            {/* Hover Glow Effect */}
            <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

            <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
                <div className={`p-3 rounded-2xl ${colorClass.replace('bg-', 'bg-opacity-10 ')} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3`}>
                    <Icon className="h-5 w-5" />
                </div>
                <div className="flex-1 min-w-0 space-y-1">
                    <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                        {preset.name}
                    </CardTitle>
                    {categoryLabel && (
                        <div className="flex items-center gap-2">
                            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
                                {categoryLabel}
                            </span>
                        </div>
                    )}
                </div>
                <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors">
                            <MoreHorizontal className="h-4 w-4" />
                            <span className="sr-only"><Trans>Open menu</Trans></span>
                        </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-48">
                        <DropdownMenuItem onClick={() => onEdit(preset)}>
                            <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
                        </DropdownMenuItem>
                        {onClone && (
                            <DropdownMenuItem onClick={() => onClone(preset)}>
                                <CopyPlus className="mr-2 h-4 w-4" /> <Trans>Clone</Trans>
                            </DropdownMenuItem>
                        )}
                        <AlertDialog>
                            <AlertDialogTrigger asChild>
                                <DropdownMenuItem className="text-destructive focus:text-destructive" onSelect={(e) => e.preventDefault()}>
                                    <Trash className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                                </DropdownMenuItem>
                            </AlertDialogTrigger>
                            <AlertDialogContent>
                                <AlertDialogHeader>
                                    <AlertDialogTitle><Trans>Delete Preset?</Trans></AlertDialogTitle>
                                    <AlertDialogDescription>
                                        <Trans>
                                            This will permanently delete the preset "{preset.name}". Pipelines using this preset may fail if not updated.
                                        </Trans>
                                    </AlertDialogDescription>
                                </AlertDialogHeader>
                                <AlertDialogFooter>
                                    <AlertDialogCancel><Trans>Cancel</Trans></AlertDialogCancel>
                                    <AlertDialogAction onClick={() => onDelete(preset.id)} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">
                                        <Trans>Delete</Trans>
                                    </AlertDialogAction>
                                </AlertDialogFooter>
                            </AlertDialogContent>
                        </AlertDialog>
                    </DropdownMenuContent>
                </DropdownMenu>
            </CardHeader>
            <CardContent className="relative pb-4 flex-1 z-10">
                {preset.description && (
                    <p className="text-xs text-muted-foreground/80 line-clamp-2 mb-4 leading-relaxed font-light">
                        {preset.description}
                    </p>
                )}
                <div className="text-sm text-muted-foreground">
                    {configKeys.length > 0 ? (
                        <div className="grid grid-cols-2 gap-2">
                            {configKeys.slice(0, 4).map(key => (
                                <div key={key} className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                                    <span className="text-[9px] uppercase tracking-wider opacity-50">{key}</span>
                                    <span className="text-[11px] font-medium truncate text-foreground/80">{String(configObj[key])}</span>
                                </div>
                            ))}
                        </div>
                    ) : (
                        <span className="italic opacity-40 text-xs"><Trans>No configuration.</Trans></span>
                    )}
                </div>
            </CardContent>
            <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
                <span className="font-mono opacity-50">#{preset.processor}</span>
            </CardFooter>
        </Card>
    );
}
