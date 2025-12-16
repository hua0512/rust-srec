import { Link, useRouter } from '@tanstack/react-router';
import { deleteSession } from '../../server/functions/sessions';
import { toast } from 'sonner';
import {
  MoreHorizontal,
  Play,
  Film,
  Clock,
  HardDrive,
  Calendar,
} from 'lucide-react';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import { Card, CardContent, CardFooter, CardHeader } from '../ui/card';
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
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';

type Session = z.infer<typeof SessionSchema>;

interface SessionCardProps {
  session: Session;
  token?: string;
}

export function SessionCard({ session, token }: SessionCardProps) {
  const router = useRouter();
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
    <Card className="flex flex-col h-full bg-linear-to-b from-card to-card/50 hover:shadow-lg hover:border-primary/20 transition-all duration-300 group">
      <CardHeader className="p-4 pb-2 flex-row gap-3 space-y-0 items-start">
        <Avatar className="h-10 w-10 border border-border shadow-2xs group-hover:border-primary/50 transition-colors">
          {session.streamer_avatar && (
            <AvatarImage
              src={session.streamer_avatar}
              alt={session.streamer_name}
              className="object-cover"
            />
          )}
          <AvatarFallback>
            {session.streamer_id.substring(0, 2).toUpperCase()}
          </AvatarFallback>
        </Avatar>
        <div className="flex-1 min-w-0">
          <div className="flex items-center justify-between">
            <p className="text-sm font-medium text-muted-foreground truncate">
              {session.streamer_name || session.streamer_id}
            </p>
            {isLive ? (
              <Badge
                variant="default"
                className="bg-red-500/10 text-red-500 hover:bg-red-500/20 border-red-500/20 animate-pulse"
              >
                <span className="relative flex h-2 w-2 mr-1.5">
                  <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-red-400 opacity-75"></span>
                  <span className="relative inline-flex rounded-full h-2 w-2 bg-red-500"></span>
                </span>
                <Trans>LIVE</Trans>
              </Badge>
            ) : (
              <Badge variant="secondary" className="text-xs font-normal">
                <Trans>Offline</Trans>
              </Badge>
            )}
          </div>
          <h3
            className="font-semibold leading-tight truncate mt-1 group-hover:text-primary transition-colors"
            title={session.title}
          >
            {session.title || <Trans>Untitled Stream</Trans>}
          </h3>
        </div>
      </CardHeader>
      <CardContent className="p-4 pt-2 grow">
        {/* Thumbnail placeholder - would be nice to have real thumbnails */}
        <div className="relative aspect-video bg-muted/50 rounded-md mb-4 overflow-hidden group-hover:ring-1 group-hover:ring-primary/20 transition-all">
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
            <Film className="h-12 w-12" />
          </div>
          {/* Gradient overlay */}
          <div className="absolute inset-0 bg-linear-to-t from-black/60 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-300 flex items-end p-3">
            <Button
              size="sm"
              variant="secondary"
              className="w-full gap-2 backdrop-blur-md bg-white/10 hover:bg-white/20 text-white border-0"
            >
              <Play className="h-3 w-3 fill-current" />{' '}
              <Trans>Watch Replay</Trans>
            </Button>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-2 text-xs text-muted-foreground">
          <div className="flex items-center gap-1.5">
            <Calendar className="h-3.5 w-3.5" />
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
            <Clock className="h-3.5 w-3.5" />
            <span>{duration}</span>
          </div>
          <div className="flex items-center gap-1.5">
            <HardDrive className="h-3.5 w-3.5" />
            <span>{formatBytes(session.total_size_bytes)}</span>
          </div>
          <div className="flex items-center gap-1.5">
            <Film className="h-3.5 w-3.5" />
            <span>
              <Trans>{session.output_count} Files</Trans>
            </span>
          </div>
        </div>
      </CardContent>
      <CardFooter className="p-2 px-4 border-t bg-muted/20 flex justify-between items-center text-xs text-muted-foreground">
        <Link
          to="/sessions/$sessionId"
          params={{ sessionId: session.id }}
          className="hover:text-primary hover:underline transition-all"
        >
          <Trans>View Details</Trans>
        </Link>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" className="h-8 w-8 -mr-2">
              <span className="sr-only">Open menu</span>
              <MoreHorizontal className="h-4 w-4" />
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

import { formatBytes, formatDuration } from '@/lib/format';
import { getMediaUrl } from '@/lib/url';
