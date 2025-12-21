import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Button } from '@/components/ui/button';
import { MoreHorizontal, Trash, Edit, Play, Pause, Video } from 'lucide-react';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { z } from 'zod';
import { StreamerSchema } from '@/api/schemas';

interface StreamActionsMenuProps {
  streamer: z.infer<typeof StreamerSchema>;
  onDelete: (id: string) => void;
  onToggle: (id: string, enabled: boolean) => void;
  onCheck: (id: string) => void;
}

export const StreamActionsMenu = ({
  streamer,
  onDelete,
  onToggle,
  onCheck: _onCheck,
}: StreamActionsMenuProps) => {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="h-7 w-7 opacity-0 group-hover:opacity-100 transition-opacity -mr-2 text-muted-foreground hover:text-foreground"
        >
          <MoreHorizontal className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuLabel>
          <Trans>Actions</Trans>
        </DropdownMenuLabel>
        {/* Hide for a now, as it is not implemented */}
        {/* <DropdownMenuItem
          onClick={() => onCheck(streamer.id)}
          className="cursor-pointer group"
        >
          <RefreshCw className="mr-2 h-4 w-4 text-primary group-hover:text-primary" />
          <span className="group-hover:text-primary transition-colors">
            <Trans>Check Now</Trans>
          </span>
        </DropdownMenuItem> */}
        <DropdownMenuItem
          onClick={() => onToggle(streamer.id, !streamer.enabled)}
          className="cursor-pointer group"
        >
          {streamer.enabled ? (
            <>
              <Pause className="mr-2 h-4 w-4 text-orange-500 group-hover:text-orange-600 dark:text-orange-400 dark:group-hover:text-orange-300" />
              <span className="text-orange-600 group-hover:text-orange-700 dark:text-orange-400 dark:group-hover:text-orange-300 transition-colors">
                <Trans>Disable</Trans>
              </span>
            </>
          ) : (
            <>
              <Play className="mr-2 h-4 w-4 text-green-500 group-hover:text-green-600 dark:text-green-400 dark:group-hover:text-green-300" />
              <span className="text-green-600 group-hover:text-green-700 dark:text-green-400 dark:group-hover:text-green-300 transition-colors">
                <Trans>Enable</Trans>
              </span>
            </>
          )}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild className="cursor-pointer group">
          <Link to="/player" search={{ url: streamer.url }}>
            <Video className="mr-2 h-4 w-4 text-purple-500 group-hover:text-purple-600 dark:text-purple-400 dark:group-hover:text-purple-300" />
            <span className="text-purple-600 group-hover:text-purple-700 dark:text-purple-400 dark:group-hover:text-purple-300 transition-colors">
              <Trans>Watch</Trans>
            </span>
          </Link>
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem asChild className="cursor-pointer group">
          <Link to="/streamers/$id/edit" params={{ id: streamer.id }}>
            <Edit className="mr-2 h-4 w-4 text-blue-500 group-hover:text-blue-600 dark:text-blue-400 dark:group-hover:text-blue-300" />
            <span className="text-blue-600 group-hover:text-blue-700 dark:text-blue-400 dark:group-hover:text-blue-300 transition-colors">
              <Trans>Edit</Trans>
            </span>
          </Link>
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={() => onDelete(streamer.id)}
          className="cursor-pointer group focus:bg-red-50 dark:focus:bg-red-950/20"
        >
          <Trash className="mr-2 h-4 w-4 text-red-500 group-hover:text-red-600" />
          <span className="text-red-600 group-hover:text-red-700">
            <Trans>Delete</Trans>
          </span>
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
};
