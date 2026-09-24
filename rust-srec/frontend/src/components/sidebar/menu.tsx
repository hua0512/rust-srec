import * as React from 'react';

import { Link, useLocation } from '@tanstack/react-router';
import { Ellipsis, LucideIcon } from 'lucide-react';
import { useLingui } from '@lingui/react';

import { cn } from '@/lib/utils';
import { getMenuList } from '@/lib/menu-list';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { CollapseMenuButton } from '@/components/sidebar/collapse-menu-button';
import { UserMenu } from '@/components/sidebar/user-menu';
import { useNotificationDot } from '@/hooks/use-notification-dot';
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from '@/components/ui/tooltip';

interface MenuProps {
  isOpen: boolean;
  className?: string;
}

interface MenuItemProps {
  href: string;
  label: string;
  icon: LucideIcon;
  isActive: boolean;
  isOpen: boolean;
  /** `undefined` renders no badge; a boolean animates it in/out. */
  showDot?: boolean;
  /** Play the slide-in; only set when the dot has just turned on. */
  dotEntering?: boolean;
  onDotEntered?: () => void;
}

const MenuItem = React.memo(function MenuItem({
  href,
  label,
  icon: Icon,
  isActive,
  isOpen,
  showDot,
  dotEntering,
  onDotEntered,
}: MenuItemProps) {
  return (
    <div className="w-full">
      <Tooltip delayDuration={100}>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            className={cn(
              'w-full h-11 mb-1 justify-start group/row relative overflow-hidden transition-[padding,color,background-color,box-shadow] duration-(--sidebar-duration) ease-(--sidebar-ease)',
              isOpen ? 'px-4' : 'pl-[calc(var(--sidebar-rail)/2-0.5rem)] pr-0',
              isActive
                ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary shadow-sm shadow-primary/5'
                : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
            )}
            asChild
          >
            <Link to={href}>
              {isActive && (
                <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1.5 h-6 bg-primary rounded-r-full" />
              )}
              <span className="relative mr-4 transition-transform duration-200 group-hover/row:scale-110 shrink-0">
                <Icon size={18} strokeWidth={isActive ? 2.5 : 2} />
                {showDot !== undefined && (
                  <span
                    aria-hidden
                    data-open={showDot}
                    data-entering={dotEntering || undefined}
                    className="rs-notification-badge absolute -top-1 -right-1"
                    onAnimationEnd={(e) => {
                      // The ping's animationend bubbles up from the child.
                      if (e.target === e.currentTarget) onDotEntered?.();
                    }}
                  >
                    <span className="rs-notification-badge-dot flex items-center justify-center">
                      <span className="rs-notification-ping absolute h-3 w-3 rounded-full bg-red-500/60 blur-[1px]" />
                      <span className="relative h-2 w-2 rounded-full bg-red-500 ring-2 ring-background shadow-[0_0_10px_rgba(239,68,68,0.6)]" />
                    </span>
                  </span>
                )}
              </span>
              <p
                className={cn(
                  'shrink-0 max-w-[calc(var(--sidebar-row)-4.5rem)] truncate font-medium transition-opacity duration-(--sidebar-duration) ease-(--sidebar-ease)',
                  !isOpen &&
                    'opacity-0 duration-(--sidebar-fade-out) pointer-events-none',
                )}
              >
                {label}
              </p>
            </Link>
          </Button>
        </TooltipTrigger>
        {!isOpen && <TooltipContent side="right">{label}</TooltipContent>}
      </Tooltip>
    </div>
  );
});

function MenuComponent({ isOpen, className }: MenuProps) {
  const { i18n } = useLingui();
  const pathname = useLocation({
    select: (location) => location.pathname,
  });
  const menuList = React.useMemo(
    () => getMenuList(pathname, i18n),
    [pathname, i18n],
  );

  const { hasCriticalDot, isPending } = useNotificationDot();
  // No badge until the first fetch settles, so a dot that is already on at
  // page load (or when the mobile sheet mounts) just appears.
  const dot = isPending ? undefined : hasCriticalDot;

  // A mount-time slide-in would replay whenever the menu mounts (e.g. the
  // mobile drawer). Instead, slide only when the dot turns on while this
  // menu is mounted, and keep the flag until the slide finishes
  // so an unrelated re-render can't cut it short. Derived during render: an
  // effect would paint the dot in place for a frame before it slides.
  const [prevDot, setPrevDot] = React.useState(dot);
  const [dotEntering, setDotEntering] = React.useState(false);
  if (dot !== prevDot) {
    setPrevDot(dot);
    setDotEntering(prevDot === false && dot === true);
  }
  const handleDotEntered = React.useCallback(() => setDotEntering(false), []);

  return (
    <ScrollArea className={cn('[&>div>div[style]]:!block', className)}>
      <nav className="mt-8 h-full w-full">
        <ul className="flex flex-col min-h-[calc(100vh-48px-36px-16px-32px)] lg:min-h-[calc(100vh-32px-40px-32px)] items-start space-y-1">
          {menuList.map(({ groupLabel, menus }, index) => (
            <li className={cn('w-full', groupLabel ? 'pt-6' : '')} key={index}>
              {groupLabel ? (
                // The heading and the collapsed-rail ellipsis share one
                // cell so the rows below never shift. One fades out before
                // the other fades in, so they never overlap.
                <div className="grid grid-cols-1 pb-3">
                  <p
                    className={cn(
                      'col-start-1 row-start-1 w-(--sidebar-row) truncate px-4 text-xs font-semibold uppercase tracking-wider text-muted-foreground/60 transition-opacity ease-(--sidebar-ease)',
                      isOpen
                        ? 'duration-(--sidebar-duration) delay-(--sidebar-fade-delay)'
                        : 'opacity-0 duration-(--sidebar-fade-out) pointer-events-none',
                    )}
                  >
                    {groupLabel}
                  </p>
                  <Tooltip delayDuration={100}>
                    <TooltipTrigger
                      tabIndex={isOpen ? -1 : undefined}
                      aria-hidden={isOpen || undefined}
                      className={cn(
                        'col-start-1 row-start-1 flex h-4 items-center pl-[calc(var(--sidebar-rail)/2-0.625rem)] transition-opacity ease-(--sidebar-ease)',
                        isOpen
                          ? 'opacity-0 duration-(--sidebar-fade-out) pointer-events-none'
                          : 'duration-(--sidebar-fade-out) delay-(--sidebar-fade-delay)',
                      )}
                    >
                      <Ellipsis className="h-5 w-5 text-muted-foreground/40" />
                    </TooltipTrigger>
                    {!isOpen && (
                      <TooltipContent side="right">
                        <p>{groupLabel}</p>
                      </TooltipContent>
                    )}
                  </Tooltip>
                </div>
              ) : (
                <div className="pb-2"></div>
              )}
              {menus.map(
                ({ href, label, icon: Icon, active, submenus }, menuIndex) =>
                  !submenus || submenus.length === 0 ? (
                    <MenuItem
                      key={menuIndex}
                      href={href}
                      label={label}
                      icon={Icon}
                      isActive={
                        active === undefined
                          ? pathname.startsWith(href)
                          : active
                      }
                      isOpen={isOpen}
                      {...(href === '/notifications' && {
                        showDot: dot,
                        dotEntering,
                        onDotEntered: handleDotEntered,
                      })}
                    />
                  ) : (
                    <div className="w-full" key={menuIndex}>
                      <CollapseMenuButton
                        icon={Icon}
                        label={label}
                        submenus={submenus}
                        isOpen={isOpen}
                      />
                    </div>
                  ),
              )}
            </li>
          ))}
          <UserMenu isOpen={isOpen} />
        </ul>
      </nav>
    </ScrollArea>
  );
}

export const Menu = React.memo(MenuComponent);
