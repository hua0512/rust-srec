import * as React from 'react';

import { Link, useRouteContext } from '@tanstack/react-router';
import { ChevronsUpDown, KeyRound, LockKeyhole, LogOut } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { cn } from '@/lib/utils';
import { Avatar, AvatarFallback } from '@/components/ui/avatar';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

interface UserMenuProps {
  isOpen: boolean | undefined;
}

/**
 * Sidebar account entry: avatar trigger + dropdown with account-scoped
 * destinations (API keys, password) and sign-out. Reads the logged-in user
 * from the `/_authed` route context populated by its `beforeLoad`.
 */
export const UserMenu = React.memo(function UserMenu({
  isOpen,
}: UserMenuProps) {
  const { user } = useRouteContext({ from: '/_authed' }) as {
    user: { username: string; roles: string[] } | null;
  };

  const username = user?.username ?? '';
  const initial = username.charAt(0).toUpperCase() || '?';
  const roles = user?.roles?.join(', ');

  return (
    <li className="w-full grow flex items-end pb-10">
      <DropdownMenu>
        <Tooltip delayDuration={100}>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                className={cn(
                  'w-full h-12 mt-5 transition-all duration-200 group',
                  isOpen === false
                    ? 'justify-center px-0'
                    : 'justify-start px-3',
                )}
              >
                <Avatar className="size-8 shrink-0 ring-1 ring-border">
                  <AvatarFallback className="bg-primary/15 text-primary text-sm font-semibold">
                    {initial}
                  </AvatarFallback>
                </Avatar>
                <div
                  className={cn(
                    'flex flex-1 items-center gap-2 min-w-0 transition-all duration-300',
                    isOpen === false
                      ? 'opacity-0 w-0 pointer-events-none'
                      : 'opacity-100 ml-3',
                  )}
                >
                  <div className="flex flex-col items-start min-w-0">
                    <span className="truncate text-sm font-medium leading-tight">
                      {username}
                    </span>
                    {roles && (
                      <span className="truncate text-[10px] text-muted-foreground leading-tight">
                        {roles}
                      </span>
                    )}
                  </div>
                  <ChevronsUpDown className="ml-auto h-4 w-4 shrink-0 text-muted-foreground" />
                </div>
              </Button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          {isOpen === false && (
            <TooltipContent side="right">{username}</TooltipContent>
          )}
        </Tooltip>
        <DropdownMenuContent
          side="top"
          align={isOpen === false ? 'center' : 'start'}
          className="w-56"
        >
          <DropdownMenuLabel className="flex flex-col">
            <span className="truncate">{username}</span>
            {roles && (
              <span className="truncate text-xs font-normal text-muted-foreground">
                {roles}
              </span>
            )}
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem asChild>
            <Link to="/config/api-keys">
              <KeyRound className="mr-2 h-4 w-4" />
              <Trans>API Keys</Trans>
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem asChild>
            <Link to="/change-password">
              <LockKeyhole className="mr-2 h-4 w-4" />
              <Trans>Change Password</Trans>
            </Link>
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem asChild variant="destructive">
            <Link to="/logout">
              <LogOut className="mr-2 h-4 w-4" />
              <Trans>Sign out</Trans>
            </Link>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </li>
  );
});
