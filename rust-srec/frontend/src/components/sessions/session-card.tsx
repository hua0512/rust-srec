import { Link, useRouter } from '@tanstack/react-router';
import { useQueryClient } from '@tanstack/react-query';
import { deleteSession } from '@/server/functions/sessions';
import { toast } from 'sonner';
import { MoreHorizontal, Film, Clock, HardDrive, Calendar } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
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
import { t } from '@lingui/core/macro';
import { formatBytes, formatDuration } from '@/lib/format';
import { getMediaUrl } from '@/lib/url';
import { getProxiedUrl } from '@/lib/utils';

type Session = z.infer<typeof SessionSchema>;

interface SessionCardProps {
  session: Session;
  token?: string;
}

export function SessionCard({ session, token }: SessionCardProps) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();
  const isLive = !session.end_time;

  const handleDelete = async () => {
    if (
      !window.confirm(
        i18n._(
          t`Are you sure you want to delete this session? This action cannot be undone.`,
        ),
      )
    ) {
      return;
    }

    try {
      await deleteSession({ data: session.id });
      toast.success(t`Session deleted successfully`);
      queryClient.invalidateQueries({ queryKey: ['sessions'] });
      router.invalidate();
    } catch (error) {
      console.error('Failed to delete session:', error);
      toast.error(t`Failed to delete session`);
    }
  };

  const duration = session.duration_secs
    ? formatDuration(session.duration_secs)
    : isLive
      ? t`Ongoing`
      : '-';

  const thumbnailUrl = getMediaUrl(session.thumbnail_url, token);

  return (
    <Card className="flex flex-col h-full bg-white/60 dark:bg-card/40 backdrop-blur-xl border-black/5 dark:border-white/5 shadow-sm dark:shadow-2xl dark:shadow-black/5 hover:shadow-md dark:hover:shadow-black/10 hover:-translate-y-1 transition-all duration-300 group relative overflow-hidden">
      <div className="absolute inset-0 bg-gradient-to-br from-primary/5 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-500 pointer-events-none" />
      <CardHeader className="p-3 pb-1.5 flex-row gap-2.5 space-y-0 items-center relative z-10">
        <Avatar className="h-8 w-8 border border-border shadow-2xs group-hover:border-primary/50 transition-colors">
          {session.streamer_avatar && (
            <AvatarImage
              src={getProxiedUrl(session.streamer_avatar)}
              alt={session.streamer_name}
              className="object-cover"
            />
          )}
          <AvatarFallback className="text-[10px]">
            {session.streamer_id.substring(0, 2).toUpperCase()}
          </AvatarFallback>
        </Avatar>
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between">
            <p className="text-xs font-medium text-muted-foreground truncate">
              {session.streamer_name || session.streamer_id}
            </p>
            {isLive ? (
              <Badge
                variant="default"
                className="bg-red-500/10 text-red-500 hover:bg-red-500/20 border-red-500/20 animate-pulse px-1.5 py-0 text-[10px] h-4"
              >
                <span className="relative flex h-1.5 w-1.5 mr-1">
                  <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75"></span>
                  <span className="relative inline-flex rounded-full h-1.5 w-1.5 bg-red-500"></span>
                </span>
                <Trans>LIVE</Trans>
              </Badge>
            ) : (
              <Badge
                variant="secondary"
                className="text-[10px] font-normal px-1.5 py-0 h-4"
              >
                <Trans>Offline</Trans>
              </Badge>
            )}
          </div>
          <h3
            className="text-sm font-semibold leading-tight truncate mt-0.5 group-hover:text-primary transition-colors"
            title={session.title}
          >
            {session.title || <Trans>Untitled Stream</Trans>}
          </h3>
        </div>
      </CardHeader>
      <CardContent className="p-3 pt-1.5 grow relative z-10">
        {/* Thumbnail placeholder - would be nice to have real thumbnails */}
        <div className="relative aspect-video bg-muted/50 rounded-sm mb-2.5 overflow-hidden group-hover:ring-1 group-hover:ring-primary/20 transition-all">
          {thumbnailUrl ? (
            <img
              src={thumbnailUrl}
              alt={t`Thumbnail for ${session.title}`}
              className="absolute inset-0 w-full h-full object-cover transition-transform duration-500 group-hover:scale-105"
              onError={(e) => {
                // Fallback to placeholder on error
                e.currentTarget.style.display = 'none';
                e.currentTarget.parentElement
                  ?.querySelector('.placeholder-icon')
                  ?.classList.remove('hidden');
              }}
            />
          ) : null}
          <div
            className={`absolute inset-0 flex items-center justify-center text-muted-foreground/30 placeholder-icon ${thumbnailUrl ? 'hidden' : ''}`}
          >
            <Film className="h-8 w-8" />
          </div>
        </div>

        <div className="grid grid-cols-2 gap-x-2 gap-y-1 text-[10px] text-muted-foreground font-medium">
          <div className="flex items-center gap-1.5">
            <Calendar className="h-3 w-3" />
            <span>
              {i18n.date(new Date(session.start_time), {
                month: 'short',
                day: 'numeric',
                hour: 'numeric',
                minute: 'numeric',
              })}
            </span>
          </div>
          <div className="flex items-center gap-1.5">
            <Clock className="h-3 w-3" />
            <span>{duration}</span>
          </div>
          <div className="flex items-center gap-1.5">
            <HardDrive className="h-3 w-3" />
            <span>{formatBytes(session.total_size_bytes)}</span>
          </div>
          <div className="flex items-center gap-1.5">
            <Film className="h-3 w-3" />
            <span>
              <Trans>{session.output_count} Files</Trans>
            </span>
          </div>
        </div>
      </CardContent>
      <CardFooter className="p-1.5 px-3 border-t border-black/5 dark:border-white/5 flex justify-between items-center text-[10px] text-muted-foreground relative z-10">
        <Link
          to="/sessions/$sessionId"
          params={{ sessionId: session.id }}
          className="hover:text-primary hover:underline transition-all"
        >
          <Trans>View Details</Trans>
        </Link>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" className="h-6 w-6 -mr-1.5">
              <span className="sr-only">Open menu</span>
              <MoreHorizontal className="h-3.5 w-3.5" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuLabel>
              <Trans>Actions</Trans>
            </DropdownMenuLabel>
            <DropdownMenuItem>
              <Link
                to="/sessions/$sessionId"
                params={{ sessionId: session.id }}
                className="flex w-full"
              >
                <Trans>View Details</Trans>
              </Link>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="text-destructive focus:text-destructive cursor-pointer"
              onClick={handleDelete}
            >
              <Trans>Delete Session</Trans>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </CardFooter>
    </Card>
  );
}
