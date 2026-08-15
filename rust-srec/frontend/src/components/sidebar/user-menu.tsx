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
                  'w-full h-12 mt-5 gap-0 transition-all duration-200 group overflow-hidden rounded-xl',
                  isOpen === false
                    ? 'justify-center px-0'
                    : 'justify-start px-2.5 hover:bg-accent/60',
                )}
              >
                <div className="relative shrink-0">
                  <Avatar className="size-9 rounded-xl border border-primary/20 bg-gradient-to-br from-primary/20 via-primary/10 to-primary/5 shadow-xs transition-all duration-300 group-hover:scale-105 group-hover:shadow-md group-hover:shadow-primary/10 group-hover:border-primary/30">
                    <AvatarFallback className="rounded-xl bg-transparent text-primary text-sm font-semibold tracking-wide select-none">
                      {initial}
                    </AvatarFallback>
                  </Avatar>
                  <span className="absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full bg-emerald-500 ring-2 ring-background ring-offset-0" />
                </div>
                <div
                  className={cn(
                    'items-center gap-2 min-w-0 transition-all duration-300',
                    isOpen === false
                      ? 'opacity-0 w-0 pointer-events-none hidden'
                      : 'flex flex-1 opacity-100 ml-3',
                  )}
                >
                  <div className="flex flex-col items-start min-w-0">
                    <span className="truncate text-sm font-medium leading-tight text-foreground/90 group-hover:text-foreground transition-colors">
                      {username}
                    </span>
                    {roles && (
                      <span className="truncate text-[10px] text-muted-foreground/80 leading-tight">
                        {roles}
                      </span>
                    )}
                  </div>
                  <ChevronsUpDown className="ml-auto h-4 w-4 shrink-0 text-muted-foreground/70 group-hover:text-muted-foreground transition-colors" />
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
