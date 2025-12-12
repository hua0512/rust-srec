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
import { Globe, Cookie, Tv, MoreHorizontal, Edit, Clock, Download } from 'lucide-react';
import { PlatformConfigSchema } from '@/api/schemas';
import { z } from 'zod';

interface PlatformCardProps {
    platform: z.infer<typeof PlatformConfigSchema>;
    onEdit: () => void;
}

// Platform-specific colors
const PLATFORM_COLORS: Record<string, string> = {
    bilibili: "bg-pink-500/10 text-pink-500 border-pink-500/20",
    douyin: "bg-cyan-500/10 text-cyan-500 border-cyan-500/20",
    douyu: "bg-orange-500/10 text-orange-500 border-orange-500/20",
    huya: "bg-yellow-500/10 text-yellow-500 border-yellow-500/20",
    twitch: "bg-purple-500/10 text-purple-500 border-purple-500/20",
    youtube: "bg-red-500/10 text-red-500 border-red-500/20",
    tiktok: "bg-slate-500/10 text-slate-500 border-slate-500/20",
    acfun: "bg-red-500/10 text-red-500 border-red-500/20",
    pandatv: "bg-blue-500/10 text-blue-500 border-blue-500/20",
    picarto: "bg-green-500/10 text-green-500 border-green-500/20",
    redbook: "bg-rose-500/10 text-rose-500 border-rose-500/20",
    twitcasting: "bg-indigo-500/10 text-indigo-500 border-indigo-500/20",
    weibo: "bg-amber-500/10 text-amber-500 border-amber-500/20",
};

export function PlatformCard({ platform, onEdit }: PlatformCardProps) {
    const colorClass = PLATFORM_COLORS[platform.name.toLowerCase()] || "bg-primary/10 text-primary border-primary/20";

    return (
        <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
            {/* Top gradient line */}
            <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

            {/* Hover Glow Effect */}
            <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

            <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
                <div className={`p-3 rounded-2xl ${colorClass.replace('bg-', 'bg-opacity-10 ')} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3`}>
                    <Globe className="h-5 w-5" />
                </div>
                <div className="flex-1 min-w-0 space-y-1">
                    <CardTitle className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                        {platform.name}
                    </CardTitle>
                    <div className="flex items-center gap-2">
                        {platform.record_danmu && (
                            <div className="flex items-center gap-1 text-[10px] uppercase tracking-wider font-semibold text-green-600 dark:text-green-400">
                                <Tv className="w-3 h-3" />
                                <span>Danmu</span>
                            </div>
                        )}
                        {platform.cookies && (
                            <div className="flex items-center gap-1 text-[10px] uppercase tracking-wider font-semibold text-orange-600 dark:text-orange-400">
                                <Cookie className="w-3 h-3" />
                                <span>Cookies</span>
                            </div>
                        )}
                    </div>
                </div>
                <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors">
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

            <CardContent className="relative pb-4 flex-1 z-10">
                <div className="grid grid-cols-2 gap-2">
                    <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                        <span className="text-[9px] uppercase tracking-wider opacity-50 flex items-center gap-1">
                            <Clock className="w-3 h-3" />
                            <Trans>Fetch</Trans>
                        </span>
                        <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                            {platform.fetch_delay_ms ? `${(platform.fetch_delay_ms / 1000).toFixed(0)}s` : <span className="opacity-50">default</span>}
                        </span>
                    </div>
                    <div className="flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                        <span className="text-[9px] uppercase tracking-wider opacity-50 flex items-center gap-1">
                            <Download className="w-3 h-3" />
                            <Trans>Download</Trans>
                        </span>
                        <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                            {platform.download_delay_ms ? `${(platform.download_delay_ms / 1000).toFixed(0)}s` : <span className="opacity-50">default</span>}
                        </span>
                    </div>
                    {platform.output_folder && (
                        <div className="col-span-2 flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                            <span className="text-[9px] uppercase tracking-wider opacity-50">
                                <Trans>Output</Trans>
                            </span>
                            <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                                {platform.output_folder}
                            </span>
                        </div>
                    )}
                    {platform.download_engine && (
                        <div className="col-span-2 flex flex-col gap-0.5 bg-muted/30 rounded-md px-2 py-1.5 border border-transparent group-hover:border-primary/5 transition-colors">
                            <span className="text-[9px] uppercase tracking-wider opacity-50">
                                <Trans>Engine</Trans>
                            </span>
                            <span className="text-[11px] font-medium truncate text-foreground/80 font-mono">
                                {platform.download_engine}
                            </span>
                        </div>
                    )}
                </div>
            </CardContent>

            <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
                <span className="font-mono opacity-50">#{platform.name.toLowerCase()}</span>
            </CardFooter>
        </Card>
    );
}
