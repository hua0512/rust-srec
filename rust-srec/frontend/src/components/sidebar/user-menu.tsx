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
  isOpen: boolean;
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
                  'w-full h-12 mt-5 gap-0 justify-start group/row overflow-hidden rounded-xl transition-[padding,color,background-color] duration-(--sidebar-duration) ease-(--sidebar-ease)',
                  isOpen
                    ? 'px-2.5 hover:bg-accent/60'
                    : 'pl-[calc(var(--sidebar-rail)/2-1.125rem)] pr-0',
                )}
              >
                <div className="relative shrink-0">
                  <Avatar className="size-9 rounded-xl border border-primary/20 bg-gradient-to-br from-primary/20 via-primary/10 to-primary/5 shadow-xs transition-all duration-300 group-hover/row:scale-105 group-hover/row:shadow-md group-hover/row:shadow-primary/10 group-hover/row:border-primary/30">
                    <AvatarFallback className="rounded-xl bg-transparent text-primary text-sm font-semibold tracking-wide select-none">
                      {initial}
                    </AvatarFallback>
                  </Avatar>
                  <span className="absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full bg-emerald-500 ring-2 ring-background ring-offset-0" />
                </div>
                <div
                  className={cn(
                    'flex shrink-0 w-[calc(var(--sidebar-row)-4.25rem)] ml-3 items-center gap-2 transition-opacity duration-(--sidebar-duration) ease-(--sidebar-ease)',
                    !isOpen &&
                      'opacity-0 duration-(--sidebar-fade-out) pointer-events-none',
                  )}
                >
                  <div className="flex flex-col items-start min-w-0">
                    <span className="truncate text-sm font-medium leading-tight text-foreground/90 group-hover/row:text-foreground transition-colors">
                      {username}
                    </span>
                    {roles && (
                      <span className="truncate text-[10px] text-muted-foreground/80 leading-tight">
                        {roles}
                      </span>
                    )}
                  </div>
                  <ChevronsUpDown className="ml-auto h-4 w-4 shrink-0 text-muted-foreground/70 group-hover/row:text-muted-foreground transition-colors" />
                </div>
              </Button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          {!isOpen && <TooltipContent side="right">{username}</TooltipContent>}
        </Tooltip>
        <DropdownMenuContent
          side="top"
          align={!isOpen ? 'center' : 'start'}
          className="w-60 p-1.5 rounded-xl border-border/50 bg-background/95 backdrop-blur-xl shadow-2xl"
        >
          <DropdownMenuLabel className="p-2 font-normal">
            <div className="flex items-center gap-3">
              <Avatar className="size-10 rounded-xl border border-primary/20 bg-gradient-to-br from-primary/25 via-primary/15 to-primary/5 shadow-xs shrink-0">
                <AvatarFallback className="rounded-xl bg-transparent text-primary text-base font-semibold tracking-wide select-none">
                  {initial}
                </AvatarFallback>
              </Avatar>
              <div className="flex flex-col min-w-0">
                <span className="truncate text-sm font-semibold text-foreground">
                  {username}
                </span>
                {roles ? (
                  <span className="inline-flex items-center self-start mt-0.5 px-1.5 py-0.5 rounded-md bg-primary/10 text-[10px] font-medium text-primary tracking-wide">
                    {roles}
                  </span>
                ) : null}
              </div>
            </div>
          </DropdownMenuLabel>
          <DropdownMenuSeparator className="my-1 bg-border/50" />
          <DropdownMenuItem asChild className="rounded-lg cursor-pointer">
            <Link to="/config/api-keys">
              <KeyRound className="mr-2 h-4 w-4 text-muted-foreground" />
              <Trans>API Keys</Trans>
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem asChild className="rounded-lg cursor-pointer">
            <Link to="/change-password">
              <LockKeyhole className="mr-2 h-4 w-4 text-muted-foreground" />
              <Trans>Change Password</Trans>
            </Link>
          </DropdownMenuItem>
          <DropdownMenuSeparator className="my-1 bg-border/50" />
          <DropdownMenuItem
            asChild
            variant="destructive"
            className="rounded-lg cursor-pointer"
          >
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
