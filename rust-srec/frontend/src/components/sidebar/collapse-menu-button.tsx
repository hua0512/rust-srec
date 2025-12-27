import { useState } from 'react';
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
  TooltipProvider,
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
  active: boolean;
  submenus: Submenu[];
  isOpen: boolean | undefined;
}

export function CollapseMenuButton({
  icon: Icon,
  label,
  active: _active,
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

  return isOpen ? (
    <Collapsible
      open={isCollapsed}
      onOpenChange={setIsCollapsed}
      className="w-full"
    >
      <CollapsibleTrigger
        className="[&[data-state=open]>div>div>svg]:rotate-180 mb-1"
        asChild
      >
        <Button
          variant="ghost"
          className={cn(
            'w-full justify-start h-11 transition-all duration-200 group relative overflow-hidden px-4 mb-1',
            isSubmenuActive
              ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary shadow-sm shadow-primary/5'
              : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
          )}
        >
          {isSubmenuActive && (
            <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1.5 h-6 bg-primary rounded-r-full" />
          )}
          <span className="transition-transform duration-200 group-hover:scale-110 shrink-0 mr-4">
            <Icon size={18} strokeWidth={isSubmenuActive ? 2.5 : 2} />
          </span>
          <p className="truncate font-medium transition-all duration-300 opacity-100 translate-x-0 w-auto">
            {label}
          </p>
          {isOpen && (
            <div className="ml-auto transition-all duration-300 translate-x-0 opacity-100">
              <ChevronDown
                size={16}
                className="transition-transform duration-200 opacity-60"
              />
            </div>
          )}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="overflow-hidden data-[state=closed]:animate-collapsible-up data-[state=open]:animate-collapsible-down">
        {submenus.map(({ href, label, active, icon: SubmenuIcon }, index) => (
          <Button
            key={index}
            variant="ghost"
            className={cn(
              'w-full justify-start h-9 mb-1 transition-all duration-200 group relative overflow-hidden px-4',
              (active === undefined && pathname === href) || active
                ? 'text-primary bg-primary/5 font-semibold'
                : 'text-muted-foreground/70 hover:bg-muted/30 hover:text-foreground',
            )}
            asChild
          >
            <Link to={href}>
              <span className="shrink-0 mr-4 ml-6 transition-transform duration-200 group-hover:scale-110">
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
              <p className="truncate text-sm transition-all duration-300 translate-x-0 opacity-100 w-auto">
                {label}
              </p>
            </Link>
          </Button>
        ))}
      </CollapsibleContent>
    </Collapsible>
  ) : (
    <DropdownMenu>
      <TooltipProvider disableHoverableContent>
        <Tooltip delayDuration={100}>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                className={cn(
                  'w-full h-11 mb-1 transition-all duration-200 group relative overflow-hidden justify-center',
                  isSubmenuActive
                    ? 'bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary shadow-sm shadow-primary/5'
                    : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
                )}
              >
                {isSubmenuActive && (
                  <div className="absolute left-0 top-1/2 -translate-y-1/2 w-1.5 h-6 bg-primary rounded-r-full" />
                )}
                <span className="transition-transform duration-200 group-hover:scale-110 shrink-0">
                  <Icon size={18} strokeWidth={isSubmenuActive ? 2.5 : 2} />
                </span>
                <p className="opacity-0 w-0 pointer-events-none">{label}</p>
              </Button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="right" align="start" alignOffset={2}>
            {label}
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
      <DropdownMenuContent
        side="right"
        sideOffset={16}
        align="start"
        className="min-w-[180px] p-2 bg-popover/95 backdrop-blur-xl border border-border/50 shadow-xl shadow-black/5"
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
                  ? 'bg-primary/10 text-primary font-medium'
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
  );
}
