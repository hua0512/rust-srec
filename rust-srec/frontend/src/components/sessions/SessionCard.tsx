import { Link, useRouter } from '@tanstack/react-router';
import { deleteSession } from '../../server/functions/sessions';
import { toast } from 'sonner';
import { format } from 'date-fns';
import { MoreHorizontal, Play, Film, Clock, HardDrive, Calendar, Trash, Eye } from 'lucide-react';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from '../ui/card';
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuLabel,
    DropdownMenuSeparator,
    DropdownMenuTrigger,
} from '../ui/dropdown-menu';
import { Avatar, AvatarFallback, AvatarImage } from '../ui/avatar';
import { SessionSchema } from '../../api/schemas';
import { z } from 'zod';
import { cn } from '@/lib/utils';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../ui/tooltip';

type Session = z.infer<typeof SessionSchema>;

interface SessionCardProps {
    session: Session;
}

export function SessionCard({ session }: SessionCardProps) {
    const router = useRouter();
    const isLive = !session.end_time;

    const handleDelete = async () => {
        if (!window.confirm('Are you sure you want to delete this session? This action cannot be undone.')) {
            return;
        }

        try {
            await deleteSession({ data: session.id });
            toast.success('Session deleted successfully');
            router.invalidate();
        } catch (error) {
            console.error('Failed to delete session:', error);
            toast.error('Failed to delete session');
        }
    };

    const duration = session.duration_secs
        ? formatDuration(session.duration_secs)
        : isLive
            ? 'Ongoing'
            : '-';

    return (
        <Card className="group overflow-hidden bg-card/50 hover:bg-card border-border/50 hover:border-primary/20 hover:shadow-xl transition-all duration-300">
            <CardHeader className="p-4 flex-row gap-3 space-y-0 items-start">
                <Avatar className="h-10 w-10 border border-border/60 shadow-sm transition-transform group-hover:scale-105">
                    {session.streamer_avatar && <AvatarImage src={session.streamer_avatar} alt={session.streamer_name} className="object-cover" />}
                    <AvatarFallback className="text-xs bg-muted text-muted-foreground font-medium">
                        {session.streamer_id.substring(0, 2).toUpperCase()}
                    </AvatarFallback>
                </Avatar>
                <div className="flex-1 min-w-0">
                    <div className="flex items-center justify-between">
                        <Link
                            to="/sessions/$sessionId"
                            params={{ sessionId: session.id }}
                            className="text-sm font-semibold truncate hover:text-primary transition-colors pr-2"
                        >
                            {session.streamer_name || session.streamer_id}
                        </Link>
                        {isLive ? (
                            <Badge variant="default" className="bg-red-500/10 text-red-500 hover:bg-red-500/20 border-red-500/20 shadow-sm">
                                <span className="relative flex h-2 w-2 mr-1.5">
                                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75"></span>
                                    <span className="relative inline-flex rounded-full h-2 w-2 bg-red-500"></span>
                                </span>
                                LIVE
                            </Badge>
                        ) : (
                            <span className="text-[10px] font-medium text-muted-foreground bg-muted/50 px-2 py-0.5 rounded-full border border-border/50">
                                {formatDuration(session.duration_secs || 0)}
                            </span>
                        )}
                    </div>
                    <TooltipProvider>
                        <Tooltip>
                            <TooltipTrigger asChild>
                                <p className="text-xs text-muted-foreground truncate mt-1 max-w-[200px]" title={session.title}>
                                    {session.title || 'Untitled Stream'}
                                </p>
                            </TooltipTrigger>
                            <TooltipContent>
                                <p className="text-xs max-w-[300px] break-words">{session.title || 'Untitled Stream'}</p>
                            </TooltipContent>
                        </Tooltip>
                    </TooltipProvider>
                </div>
            </CardHeader>

            <CardContent className="p-0">
                <div className="relative aspect-video bg-muted/30 group-hover:bg-muted/50 transition-colors overflow-hidden">
                    {session.thumbnail_url ? (
                        <img
                            src={session.thumbnail_url}
                            alt={`Thumbnail for ${session.title}`}
                            className="absolute inset-0 w-full h-full object-cover transition-transform duration-700 group-hover:scale-105"
                            onError={(e) => {
                                e.currentTarget.style.display = 'none';
                                e.currentTarget.parentElement?.querySelector('.placeholder-icon')?.classList.remove('hidden');
                            }}
                        />
                    ) : null}
                    <div className={cn(
                        "absolute inset-0 flex items-center justify-center text-muted-foreground/20 placeholder-icon transition-opacity duration-300",
                        session.thumbnail_url ? 'hidden' : ''
                    )}>
                        <Film className="h-12 w-12" />
                    </div>

                    {/* Overlay Actions */}
                    <div className="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity duration-300 flex items-center justify-center gap-2 backdrop-blur-[1px]">
                        <Button size="sm" variant="secondary" className="gap-2 h-8 bg-white/10 hover:bg-white/20 text-white border-white/10 backdrop-blur-md" asChild>
                            <Link to="/sessions/$sessionId" params={{ sessionId: session.id }}>
                                <Play className="h-3.5 w-3.5 fill-current" /> Details
                            </Link>
                        </Button>
                    </div>

                    {/* Bottom Metadata Bar */}
                    <div className="absolute bottom-0 left-0 right-0 p-2 bg-gradient-to-t from-black/80 to-transparent flex justify-between items-end text-[10px] text-white/90 font-medium">
                        <div className="flex items-center gap-2">
                            <div className="flex items-center gap-1 bg-black/40 px-1.5 py-0.5 rounded backdrop-blur-sm">
                                <Calendar className="h-3 w-3 text-white/70" />
                                <span>{format(new Date(session.start_time), 'MMM d, HH:mm')}</span>
                            </div>
                        </div>
                        <div className="flex items-center gap-1 bg-black/40 px-1.5 py-0.5 rounded backdrop-blur-sm">
                            <HardDrive className="h-3 w-3 text-white/70" />
                            <span>{formatBytes(session.total_size_bytes)}</span>
                        </div>
                    </div>
                </div>
            </CardContent>

            <CardFooter className="p-2 px-3 flex justify-between items-center text-xs border-t border-border/40 bg-muted/5">
                <div className="flex items-center gap-1.5 text-muted-foreground">
                    <Film className="h-3.5 w-3.5 opacity-70" />
                    <span>{session.output_count} Segments</span>
                </div>

                <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="icon" className="h-7 w-7 text-muted-foreground hover:text-foreground">
                            <MoreHorizontal className="h-4 w-4" />
                        </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end" className="w-48">
                        <DropdownMenuLabel>Actions</DropdownMenuLabel>
                        <DropdownMenuItem asChild className="cursor-pointer text-primary focus:text-primary focus:bg-primary/10">
                            <Link to="/sessions/$sessionId" params={{ sessionId: session.id }}>
                                <Eye className="mr-2 h-4 w-4" /> View Details
                            </Link>
                        </DropdownMenuItem>
                        <DropdownMenuSeparator />
                        <DropdownMenuItem
                            className="text-red-600 focus:text-red-700 focus:bg-red-50 dark:focus:bg-red-950/20 cursor-pointer"
                            onClick={handleDelete}
                        >
                            <Trash className="mr-2 h-4 w-4" /> Delete Session
                        </DropdownMenuItem>
                    </DropdownMenuContent>
                </DropdownMenu>
            </CardFooter>
        </Card >
    );
}

function formatDuration(seconds: number): string {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const secs = seconds % 60;

    const parts = [];
    if (hours > 0) parts.push(`${hours}h`);
    if (minutes > 0) parts.push(`${minutes}m`);
    parts.push(`${secs}s`);

    return parts.join(' ');
}

function formatBytes(bytes: number, decimals = 2) {
    if (!+bytes) return '0 B';

    const k = 1024;
    const dm = decimals < 0 ? 0 : decimals;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];

    const i = Math.floor(Math.log(bytes) / Math.log(k));

    return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}
