import {
    Card,
    CardContent,
    CardFooter,
    CardHeader,
    CardTitle,
} from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Trans } from '@lingui/react/macro';
import { Cookie, Tv, MoreHorizontal, Edit, Clock, Download } from 'lucide-react';
import { PlatformConfigSchema } from '@/api/schemas';
import { z } from 'zod';
import { getPlatformIcon, getPlatformColor } from "@/components/pipeline/constants";
import { cn } from "@/lib/utils";

interface PlatformCardProps {
    platform: z.infer<typeof PlatformConfigSchema>;
    onEdit: () => void;
}

export function PlatformCard({ platform, onEdit }: PlatformCardProps) {
    const Icon = getPlatformIcon(platform.name);
    const colorClass = getPlatformColor(platform.name);

    return (
        <Card className="h-full flex flex-col border-border/50 bg-card/50 backdrop-blur-sm shadow-sm hover:shadow-md transition-all duration-300 hover:border-primary/20 group relative overflow-hidden">
            {/* Hover Glow Effect */}
            <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

            <CardHeader className="flex flex-row items-center gap-4 pb-2 space-y-0 relative z-10 px-4 pt-4">
                <div className={cn(
                    "p-3 rounded-xl ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-105",
                    colorClass
                )}>
                    <Icon className="h-5 w-5" />
                </div>
                <div className="flex-1 min-w-0 space-y-1">
                    <CardTitle className="text-base font-semibold tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                        {platform.name}
                    </CardTitle>
                    <div className="flex items-center gap-2">
                        {platform.record_danmu && (
                            <div className="flex items-center gap-1 text-[10px] uppercase tracking-wider font-semibold text-green-600 dark:text-green-400 bg-green-500/10 px-1.5 py-0.5 rounded-full border border-green-500/10">
                                <Tv className="w-3 h-3" />
                                <span>Danmu</span>
                            </div>
                        )}
                        {platform.cookies && (
                            <div className="flex items-center gap-1 text-[10px] uppercase tracking-wider font-semibold text-orange-600 dark:text-orange-400 bg-orange-500/10 px-1.5 py-0.5 rounded-full border border-orange-500/10">
                                <Cookie className="w-3 h-3" />
                                <span>Cookies</span>
                            </div>
                        )}
                    </div>
                </div>
                <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-8 w-8 -mr-2 text-muted-foreground/70 hover:text-foreground transition-colors">
                            <MoreHorizontal className="h-4 w-4" />
                            <span className="sr-only"><Trans>Open menu</Trans></span>
                        </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-48">
                        <DropdownMenuItem onClick={onEdit}>
                            <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
                        </DropdownMenuItem>
                    </DropdownMenuContent>
                </DropdownMenu>
            </CardHeader>

            <CardContent className="relative pb-4 flex-1 z-10 pt-4 px-4">
                <div className="grid grid-cols-2 gap-3">
                    <div className="flex flex-col gap-0.5 bg-muted/40 rounded-lg px-3 py-2 border border-border/50 group-hover:border-primary/10 transition-colors">
                        <span className="text-[10px] uppercase tracking-wider opacity-60 flex items-center gap-1.5 font-medium">
                            <Clock className="w-3 h-3" />
                            <Trans>Fetch</Trans>
                        </span>
                        <span className="text-xs font-semibold truncate text-foreground/90 font-mono">
                            {platform.fetch_delay_ms ? `${(platform.fetch_delay_ms / 1000).toFixed(0)}s` : <span className="opacity-50">default</span>}
                        </span>
                    </div>
                    <div className="flex flex-col gap-0.5 bg-muted/40 rounded-lg px-3 py-2 border border-border/50 group-hover:border-primary/10 transition-colors">
                        <span className="text-[10px] uppercase tracking-wider opacity-60 flex items-center gap-1.5 font-medium">
                            <Download className="w-3 h-3" />
                            <Trans>Wait</Trans>
                        </span>
                        <span className="text-xs font-semibold truncate text-foreground/90 font-mono">
                            {platform.download_delay_ms ? `${(platform.download_delay_ms / 1000).toFixed(0)}s` : <span className="opacity-50">default</span>}
                        </span>
                    </div>
                    {platform.output_folder && (
                        <div className="col-span-2 flex flex-col gap-0.5 bg-muted/40 rounded-lg px-3 py-2 border border-border/50 group-hover:border-primary/10 transition-colors">
                            <span className="text-[10px] uppercase tracking-wider opacity-60 font-medium">
                                <Trans>Output</Trans>
                            </span>
                            <span className="text-xs font-semibold truncate text-foreground/90 font-mono" title={platform.output_folder}>
                                {platform.output_folder}
                            </span>
                        </div>
                    )}
                    {platform.download_engine && (
                        <div className="col-span-2 flex flex-col gap-0.5 bg-muted/40 rounded-lg px-3 py-2 border border-border/50 group-hover:border-primary/10 transition-colors">
                            <span className="text-[10px] uppercase tracking-wider opacity-60 font-medium">
                                <Trans>Engine</Trans>
                            </span>
                            <span className="text-xs font-semibold truncate text-foreground/90 font-mono">
                                {platform.download_engine}
                            </span>
                        </div>
                    )}
                </div>
            </CardContent>

            <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/30 mt-auto px-6 py-3 bg-muted/20">
                <span className="font-mono opacity-50">#{platform.name.toLowerCase()}</span>
            </CardFooter>
        </Card>
    );
}
