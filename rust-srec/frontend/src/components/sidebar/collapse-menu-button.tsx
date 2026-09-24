import { memo, useRef, useState } from 'react';
import { ChevronDown, Dot, LucideIcon } from 'lucide-react';
import { Link, useLocation } from '@tanstack/react-router';

import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { DropdownMenuArrow } from '@radix-ui/react-dropdown-menu';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from '@/components/ui/tooltip';
import {
  DropdownMenu,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu';

type Submenu = {
  href: string;
  label: string;
  active?: boolean;
  icon?: LucideIcon;
};

interface CollapseMenuButtonProps {
  icon: LucideIcon;
  label: string;
  submenus: Submenu[];
  isOpen: boolean;
}

export const CollapseMenuButton = memo(function CollapseMenuButton({
  icon: Icon,
  label,
  submenus,
  isOpen,
}: CollapseMenuButtonProps) {
  const pathname = useLocation({
    select: (location) => location.pathname,
  });
  const isSubmenuActive = submenus.some((submenu) =>
    submenu.active === undefined ? submenu.href === pathname : submenu.active,
  );
  const [isCollapsed, setIsCollapsed] = useState<boolean>(isSubmenuActive);
  const [menuOpen, setMenuOpen] = useState(false);
  if (isOpen && menuOpen) {
    setMenuOpen(false);
  }
  const buttonRef = useRef<HTMLButtonElement>(null);

  return (
    <Collapsible
      open={isOpen && isCollapsed}
      onOpenChange={setIsCollapsed}
      className="relative w-full"
    >
      {/* In the collapsed rail the submenu opens as a dropdown instead. Its
          trigger is an inert overlay, so the visible button below stays the
          same element in both states and animates with the sidebar. */}
      <DropdownMenu open={!isOpen && menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger asChild>
          <span
            aria-hidden
            className="pointer-events-none absolute inset-x-0 top-0 h-11"
          >
            <span className="sr-only">{label}</span>
          </span>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          side="right"
          sideOffset={16}
          align="start"
          className="min-w-[180px] p-2 bg-popover/95 backdrop-blur-xl border border-border/50 shadow-xl shadow-black/5"
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            buttonRef.current?.focus();
          }}
        >
          <DropdownMenuLabel className="px-2 py-1.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground/70">
            {label}
          </DropdownMenuLabel>
          <DropdownMenuSeparator className="my-1.5 bg-border/50" />
          {submenus.map(({ href, label, active, icon: SubmenuIcon }, index) => (
            <DropdownMenuItem
              key={index}
              asChild
              className="p-0 focus:bg-transparent"
            >
              <Link
                className={cn(
                  'flex items-center w-full px-3 py-2 rounded-md cursor-pointer transition-all duration-200 group',
                  (active === undefined && pathname === href) || active
                    ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary font-medium'
                    : 'text-foreground/80 hover:bg-muted/50 hover:text-foreground',
                )}
                to={href}
              >
                {SubmenuIcon && (
                  <SubmenuIcon
                    size={16}
                    className="mr-3 shrink-0 transition-transform duration-200 group-hover:scale-110"
                  />
                )}
                <p className="truncate text-sm">{label}</p>
                {((active === undefined && pathname === href) || active) && (
                  <div className="ml-auto w-1.5 h-1.5 rounded-full bg-primary" />
                )}
              </Link>
            </DropdownMenuItem>
          ))}
          <DropdownMenuArrow className="fill-popover" />
        </DropdownMenuContent>
      </DropdownMenu>
      <Tooltip delayDuration={100}>
        <TooltipTrigger asChild>
          <CollapsibleTrigger asChild>
            <Button
              ref={buttonRef}
              variant="ghost"
              onClick={(event) => {
                if (isOpen) return;
                // Stops the collapsible from toggling behind the dropdown.
                event.preventDefault();
                setMenuOpen(true);
              }}
              {...(!isOpen && {
                'aria-haspopup': 'menu',
                'aria-expanded': menuOpen,
              })}
              className={cn(
                'w-full h-11 mb-1 justify-start group/row relative overflow-hidden transition-[padding,color,background-color,box-shadow] duration-(--sidebar-duration) ease-(--sidebar-ease)',
                isOpen
                  ? 'px-4'
                  : 'pl-[calc(var(--sidebar-rail)/2-0.5rem)] pr-0',
                isSubmenuActive
                  ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary shadow-sm shadow-primary/5'
                  : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
              )}
            >
              {isSubmenuActive && (
                <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1.5 h-6 bg-primary rounded-r-full" />
              )}
              <span className="transition-transform duration-200 group-hover/row:scale-110 shrink-0 mr-4">
                <Icon size={18} strokeWidth={isSubmenuActive ? 2.5 : 2} />
              </span>
              <p
                className={cn(
                  'shrink-0 max-w-[calc(var(--sidebar-row)-6rem)] truncate font-medium transition-opacity duration-(--sidebar-duration) ease-(--sidebar-ease)',
                  !isOpen && 'opacity-0 duration-(--sidebar-fade-out)',
                )}
              >
                {label}
              </p>
              <span
                className={cn(
                  'absolute top-1/2 left-[calc(var(--sidebar-row)-2rem)] -translate-y-1/2 opacity-60 transition-[rotate,opacity] duration-(--sidebar-duration) ease-(--sidebar-ease)',
                  isOpen && isCollapsed && 'rotate-180',
                  !isOpen && 'opacity-0 duration-(--sidebar-fade-out)',
                )}
              >
                <ChevronDown size={16} />
              </span>
            </Button>
          </CollapsibleTrigger>
        </TooltipTrigger>
        {!isOpen && (
          <TooltipContent side="right" align="start" alignOffset={2}>
            {label}
          </TooltipContent>
        )}
      </Tooltip>
      <CollapsibleContent className="overflow-hidden data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down duration-(--sidebar-duration) ease-(--sidebar-ease)">
        {submenus.map(({ href, label, active, icon: SubmenuIcon }, index) => (
          <Button
            key={index}
            variant="ghost"
            className={cn(
              'w-full justify-start h-9 mb-1 transition-all duration-200 group/row relative overflow-hidden px-4',
              (active === undefined && pathname === href) || active
                ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary font-semibold'
                : 'text-muted-foreground/70 hover:bg-muted/50 hover:text-foreground',
            )}
            asChild
          >
            <Link to={href}>
              <span className="shrink-0 mr-4 ml-6 transition-transform duration-200 group-hover/row:scale-110">
                {SubmenuIcon ? (
                  <SubmenuIcon size={16} />
                ) : (
                  <Dot
                    size={18}
                    className={cn(
                      (active === undefined && pathname === href) || active
                        ? 'opacity-100 scale-125'
                        : 'opacity-40',
                    )}
                  />
                )}
              </span>
              <p className="shrink-0 max-w-[calc(var(--sidebar-row)-6.125rem)] truncate text-sm">
                {label}
              </p>
            </Link>
          </Button>
        ))}
      </CollapsibleContent>
    </Collapsible>
  );
});
