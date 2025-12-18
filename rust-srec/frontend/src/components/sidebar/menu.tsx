import * as React from 'react';

import { Link, useLocation } from '@tanstack/react-router';
import { Ellipsis, LogOut } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { cn } from '@/lib/utils';
import { getMenuList } from '@/lib/menu-list';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { CollapseMenuButton } from '@/components/sidebar/collapse-menu-button';
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from '@/components/ui/tooltip';

interface MenuProps {
  isOpen: boolean | undefined;
  className?: string;
}

export function MenuComponent({ isOpen, className }: MenuProps) {
  const pathname = useLocation({
    select: (location) => location.pathname,
  });
  const menuList = getMenuList(pathname);

  return (
    <ScrollArea className={cn('[&>div>div[style]]:!block', className)}>
      <nav className="mt-8 h-full w-full">
        <ul
          className={cn(
            'flex flex-col min-h-[calc(100vh-48px-36px-16px-32px)] lg:min-h-[calc(100vh-32px-40px-32px)] items-start space-y-1',
            isOpen === false ? 'px-1' : 'px-2',
          )}
        >
          {menuList.map(({ groupLabel, menus }, index) => (
            <li className={cn('w-full', groupLabel ? 'pt-6' : '')} key={index}>
              {(isOpen && groupLabel) || isOpen === undefined ? (
                <p className="text-xs font-semibold uppercase tracking-wider text-muted-foreground/60 px-4 pb-3 max-w-[248px] truncate">
                  {groupLabel}
                </p>
              ) : !isOpen && isOpen !== undefined && groupLabel ? (
                <TooltipProvider>
                  <Tooltip delayDuration={100}>
                    <TooltipTrigger className="w-full">
                      <div className="w-full flex justify-center items-center py-2">
                        <Ellipsis className="h-5 w-5 text-muted-foreground/40" />
                      </div>
                    </TooltipTrigger>
                    <TooltipContent side="right">
                      <p>{groupLabel}</p>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              ) : (
                <div className="pb-2"></div>
              )}
              {menus.map(
                ({ href, label, icon: Icon, active, submenus }, index) =>
                  !submenus || submenus.length === 0 ? (
                    <div className="w-full" key={index}>
                      <TooltipProvider disableHoverableContent>
                        <Tooltip delayDuration={100}>
                          <TooltipTrigger asChild>
                            <Button
                              variant="ghost"
                              className={cn(
                                'w-full h-11 mb-1 transition-all duration-200 group relative overflow-hidden',
                                isOpen === false
                                  ? 'justify-center'
                                  : 'justify-start px-4',
                                (active === undefined &&
                                  pathname.startsWith(href)) ||
                                  active
                                  ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary shadow-sm shadow-primary/5'
                                  : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
                              )}
                              asChild
                            >
                              <Link to={href}>
                                {((active === undefined &&
                                  pathname.startsWith(href)) ||
                                  active) && (
                                  <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1.5 h-6 bg-primary rounded-r-full" />
                                )}
                                <span
                                  className={cn(
                                    'transition-transform duration-200 group-hover:scale-110 shrink-0',
                                    isOpen === false ? '' : 'mr-4',
                                  )}
                                >
                                  <Icon
                                    size={18}
                                    strokeWidth={
                                      (active === undefined &&
                                        pathname.startsWith(href)) ||
                                      active
                                        ? 2.5
                                        : 2
                                    }
                                  />
                                </span>
                                <p
                                  className={cn(
                                    'truncate font-medium transition-all duration-300',
                                    isOpen === false
                                      ? 'opacity-0 w-0 pointer-events-none'
                                      : 'opacity-100 translate-x-0 w-auto',
                                  )}
                                >
                                  {label}
                                </p>
                              </Link>
                            </Button>
                          </TooltipTrigger>
                          {isOpen === false && (
                            <TooltipContent side="right">
                              {label}
                            </TooltipContent>
                          )}
                        </Tooltip>
                      </TooltipProvider>
                    </div>
                  ) : (
                    <div className="w-full" key={index}>
                      <CollapseMenuButton
                        icon={Icon}
                        label={label}
                        active={
                          active === undefined
                            ? pathname.startsWith(href)
                            : active
                        }
                        submenus={submenus}
                        isOpen={isOpen}
                      />
                    </div>
                  ),
              )}
            </li>
          ))}
          <li className="w-full grow flex items-end pb-4">
            <TooltipProvider disableHoverableContent>
              <Tooltip delayDuration={100}>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    className={cn(
                      'w-full h-11 mt-5 bg-destructive/5 text-destructive hover:bg-destructive/10 hover:text-destructive transition-all duration-200 border-none group relative overflow-hidden',
                      isOpen === false
                        ? 'justify-center'
                        : 'justify-start px-4',
                    )}
                    asChild
                  >
                    <Link to="/logout">
                      <span
                        className={cn(
                          'transition-transform duration-200 group-hover:scale-110 shrink-0',
                          isOpen === false ? '' : 'mr-4',
                        )}
                      >
                        <LogOut size={18} />
                      </span>
                      <p
                        className={cn(
                          'whitespace-nowrap font-medium transition-all duration-300',
                          isOpen === false
                            ? 'opacity-0 w-0 pointer-events-none'
                            : 'opacity-100 translate-x-0 w-auto',
                        )}
                      >
                        <Trans>Sign out</Trans>
                      </p>
                    </Link>
                  </Button>
                </TooltipTrigger>
                {isOpen === false && (
                  <TooltipContent side="right">
                    <Trans>Sign out</Trans>
                  </TooltipContent>
                )}
              </Tooltip>
            </TooltipProvider>
          </li>
        </ul>
      </nav>
    </ScrollArea>
  );
}

export const Menu = React.memo(MenuComponent);
