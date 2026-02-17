import { memo } from 'react';
import { Link, useRouter } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { deleteSession } from '@/server/functions/sessions';
import { toast } from 'sonner';
import {
  MoreHorizontal,
  Clock,
  HardDrive,
  Calendar,
  PlayCircle,
  Eye,
  Trash2,
  ChevronRight,
  Check,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { SessionSchema } from '@/api/schemas';
import { z } from 'zod';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { formatBytes, formatDuration } from '@/lib/format';
import { cn, getProxiedUrl } from '@/lib/utils';
import { motion } from 'motion/react';

type Session = z.infer<typeof SessionSchema>;

interface SessionCardProps {
  session: Session;
  token?: string;
  selectionMode?: boolean;
  isSelected?: boolean;
  onSelectChange?: (id: string, selected: boolean) => void;
}

export const SessionCard = memo(
  ({
    session,
    selectionMode,
    isSelected,
    onSelectChange,
  }: SessionCardProps) => {
    const router = useRouter();
    const queryClient = useQueryClient();
    const { i18n } = useLingui();
    const isLive = !session.end_time;

    const handleCardClick = (e: React.MouseEvent) => {
      if (selectionMode && onSelectChange) {
        e.preventDefault();
        e.stopPropagation();
        onSelectChange(session.id, !isSelected);
      }
    };

    const handleDelete = async () => {
      if (
        !window.confirm(
          i18n._(
            msg`Are you sure you want to delete this session? This action cannot be undone.`,
          ),
        )
      ) {
        return;
      }

      try {
        await deleteSession({ data: session.id });
        toast.success(i18n._(msg`Session deleted successfully`));
        void queryClient.invalidateQueries({ queryKey: ['sessions'] });
        void router.invalidate();
      } catch (error) {
        console.error('Failed to delete session:', error);
        toast.error(i18n._(msg`Failed to delete session`));
      }
    };

    const duration = session.duration_secs
      ? formatDuration(session.duration_secs)
      : isLive
        ? i18n._(msg`Ongoing`)
        : '-';

    return (
      <Card
        className={cn(
          'flex flex-col h-full bg-card border border-border/50 shadow-[0_2px_12px_rgba(0,0,0,0.08)] hover:shadow-[0_8px_30px_rgba(0,0,0,0.15)] hover:-translate-y-0.5 hover:border-primary/25 transition-[transform,box-shadow,border-color] duration-300 ease-out group relative overflow-hidden rounded-2xl',
          selectionMode && 'cursor-pointer',
          isSelected && 'ring-2 ring-primary border-primary/50',
        )}
        onClick={handleCardClick}
      >
        {/* Subtle corner tint */}
        <div className="absolute top-0 left-0 w-1/2 h-1/2 bg-gradient-to-br from-primary/[0.03] to-transparent pointer-events-none" />

        {/* Selection Checkbox Indicator */}
        {selectionMode && (
          <div className="absolute top-3.5 right-3.5 z-30">
            <motion.div
              initial={false}
              animate={{
                scale: isSelected ? 1 : 0.85,
                opacity: isSelected ? 1 : 0.6,
              }}
              whileHover={{ scale: 1, opacity: 1 }}
              className={cn(
                'h-6 w-6 rounded-full border-2 flex items-center justify-center transition-all duration-500 shadow-xl backdrop-blur-md',
                isSelected
                  ? 'bg-primary border-primary text-primary-foreground shadow-primary/40 ring-4 ring-primary/20'
                  : 'bg-black/40 border-white/20 text-transparent',
              )}
            >
              <Check
                className={cn(
                  'h-3.5 w-3.5 transition-transform duration-500',
                  isSelected ? 'scale-100' : 'scale-0',
                )}
              />
            </motion.div>
          </div>
        )}

        <CardHeader className="p-3.5 pb-0 flex-row gap-3 space-y-0 items-start relative z-10">
          <div className="relative shrink-0 pt-0.5">
            <Avatar className="h-10 w-10 border-2 border-white/10 shadow-xl transition-colors duration-300 group-hover:border-primary/30 ring-4 ring-black/5">
              {session.streamer_avatar && (
                <AvatarImage
                  src={getProxiedUrl(session.streamer_avatar)}
                  alt={session.streamer_name}
                  className="object-cover"
                />
              )}
              <AvatarFallback className="text-[9px] font-black bg-gradient-to-br from-muted to-muted/50 text-muted-foreground uppercase tracking-tighter">
                {session.streamer_name?.substring(0, 2).toUpperCase() || '??'}
              </AvatarFallback>
            </Avatar>
          </div>

          <div className="flex-1 min-w-0 pt-0.5">
            <div className="flex items-center justify-between gap-2 mb-0.5">
              <p className="text-xs font-black tracking-tight text-primary/80 uppercase truncate">
                {session.streamer_name}
              </p>
              {isLive ? (
                <div className="flex items-center gap-1 px-1.5 py-0.5 rounded-full bg-red-500/10 border border-red-500/20 shadow-[0_0_12px_rgba(239,68,68,0.15)] animate-pulse">
                  <span className="relative flex h-1 w-1">
                    <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75"></span>
                    <span className="relative inline-flex rounded-full h-1 w-1 bg-red-500"></span>
                  </span>
                  <span className="text-[7.5px] font-black tracking-widest text-red-500">
                    LIVE
                  </span>
                </div>
              ) : (
                <div className="px-1.5 py-0.5 rounded-full bg-muted/10 border border-white/5">
                  <span className="text-[7.5px] font-black tracking-widest text-muted-foreground/50">
                    ENDED
                  </span>
                </div>
              )}
            </div>
            <h3
              className="text-[14px] font-bold leading-tight tracking-tight text-foreground/90 line-clamp-2 transition-colors duration-300 group-hover:text-foreground min-h-[1.25rem]"
              title={session.title}
            >
              {session.title || <Trans>Untitled Stream</Trans>}
            </h3>
          </div>
        </CardHeader>

        <CardContent className="p-4 pt-1 pb-4 relative z-10 flex-1">
          <div className="flex flex-col gap-2.5">
            <div className="flex items-center gap-2 text-muted-foreground/40">
              <Calendar className="h-3 w-3 text-blue-400/70" />
              <span className="text-[10px] font-black uppercase tracking-widest leading-none">
                {i18n.date(new Date(session.start_time), {
                  month: 'short',
                  day: 'numeric',
                  hour: '2-digit',
                  minute: '2-digit',
                  second: '2-digit',
                })}
              </span>
            </div>

            <div className="flex items-center gap-4 text-[10px] font-black tabular-nums text-foreground/70">
              <div className="flex items-center gap-1.5 min-w-0">
                <Clock className="h-3 w-3 text-purple-400/70 shrink-0" />
                <span className="truncate">{duration}</span>
              </div>

              <div className="w-[1px] h-2.5 bg-white/5 shrink-0" />

              <div className="flex items-center gap-1.5 min-w-0">
                <HardDrive className="h-3 w-3 text-amber-400/70 shrink-0" />
                <span className="truncate">
                  {formatBytes(session.total_size_bytes)}
                </span>
              </div>
            </div>
          </div>
        </CardContent>

        <CardFooter className="p-3 pt-0 flex justify-between items-center relative z-10">
          <Button
            variant="ghost"
            size="sm"
            asChild
            className="h-9 px-4 rounded-xl text-[10px] font-black uppercase tracking-[0.2em] text-muted-foreground/70 hover:text-primary hover:bg-primary/5 border border-transparent hover:border-primary/20 transition-all duration-500 group/btn"
          >
            <Link
              to="/sessions/$sessionId"
              params={{ sessionId: session.id }}
              className="flex items-center"
            >
              <PlayCircle className="w-4 h-4 mr-2.5 transition-all duration-500 group-hover/btn:scale-110 group-hover/btn:rotate-[360deg] text-primary/60 group-hover/btn:text-primary" />
              <Trans>Explore</Trans>
              <ChevronRight className="w-3 h-3 ml-1 opacity-0 -translate-x-2 group-hover/btn:opacity-100 group-hover/btn:translate-x-0 transition-all duration-500" />
            </Link>
          </Button>

          <div className="flex items-center gap-1">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-9 w-9 text-muted-foreground/30 hover:text-foreground hover:bg-white/5 rounded-xl transition-all duration-500"
                >
                  <MoreHorizontal className="h-4.5 w-4.5" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                align="end"
                className="w-52 p-1.5 bg-card/40 backdrop-blur-3xl border-white/10 rounded-2xl shadow-[0_20px_50px_rgba(0,0,0,0.3)] animate-in fade-in zoom-in-95 duration-200"
              >
                <DropdownMenuLabel className="px-3 py-2 text-[9px] font-black uppercase tracking-[0.2em] text-muted-foreground/40">
                  <Trans>Session Menu</Trans>
                </DropdownMenuLabel>
                <DropdownMenuSeparator className="bg-white/5 mx-1 my-1" />

                <DropdownMenuItem asChild>
                  <Link
                    to="/sessions/$sessionId"
                    params={{ sessionId: session.id }}
                    className="flex items-center justify-between px-3 py-2.5 rounded-xl cursor-pointer transition-all focus:bg-primary/10 focus:text-primary group/item"
                  >
                    <div className="flex items-center gap-3">
                      <div className="p-1.5 rounded-lg bg-primary/10 text-primary/60 group-hover/item:scale-110 transition-transform">
                        <Eye className="h-3.5 w-3.5" />
                      </div>
                      <span className="font-bold text-xs">
                        <Trans>View Details</Trans>
                      </span>
                    </div>
                  </Link>
                </DropdownMenuItem>

                <DropdownMenuSeparator className="bg-white/5 mx-1 my-1" />

                <DropdownMenuItem
                  className="flex items-center justify-between px-3 py-2.5 rounded-xl text-destructive focus:text-destructive focus:bg-destructive/10 cursor-pointer group/del"
                  onClick={handleDelete}
                >
                  <div className="flex items-center gap-3">
                    <div className="p-1.5 rounded-lg bg-destructive/10 text-destructive/60 group-hover/del:scale-110 transition-transform">
                      <Trash2 className="h-3.5 w-3.5" />
                    </div>
                    <span className="font-bold text-xs">
                      <Trans>Delete</Trans>
                    </span>
                  </div>
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </CardFooter>
      </Card>
    );
  },
);
