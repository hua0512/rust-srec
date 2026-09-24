import type React from 'react';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';

import { Menu } from '@/components/sidebar/menu';
import { SidebarToggle } from '@/components/sidebar/sidebar-toggle';
import { Button } from '@/components/ui/button';
import { Sidebar, useSidebar } from '@/components/ui/sidebar';
import { cn } from '@/lib/utils';

export function AppSidebar({
  side = 'left',
  collapsible = 'offcanvas',
  ...props
}: React.ComponentProps<typeof Sidebar>) {
  const { state, open, toggleSidebar, isMobile } = useSidebar();
  const isOpen = isMobile ? true : state === 'expanded';

  return (
    <Sidebar side={side} collapsible={collapsible} {...props}>
      {collapsible === 'icon' && !isMobile ? (
        <SidebarToggle isOpen={open} setIsOpen={toggleSidebar} side={side} />
      ) : null}

      {/* Rows keep their expanded layout while the sidebar narrows, so the
          edge clips them instead of reflowing them; only the leading icon's
          padding moves it to the centre of the collapsed rail. --sidebar-row
          and --sidebar-rail are a row's expanded and collapsed widths (minus
          px-3 and the container border). */}
      <div className="relative h-full flex flex-col py-4 px-3 overflow-hidden [--sidebar-row:calc(var(--sidebar-width)-1.5rem-1px)] [--sidebar-rail:calc(var(--sidebar-width-icon)-1.5rem-1px)]">
        <Button
          className={cn(
            'w-full justify-start mb-6 bg-transparent hover:bg-transparent transition-[padding] duration-(--sidebar-duration) ease-(--sidebar-ease)',
            isOpen ? 'px-4' : 'pl-[calc(var(--sidebar-rail)/2-1.25rem)] pr-0',
          )}
          variant="link"
          asChild
        >
          <Link to="/dashboard" className="flex items-center gap-4">
            <div className="w-10 h-10 bg-primary rounded-xl flex items-center justify-center shrink-0 shadow-lg shadow-primary/20">
              <div className="w-6 h-6 bg-primary-foreground [mask-image:url(/stream-rec-white.svg)] [mask-size:contain] [mask-repeat:no-repeat] [mask-position:center]" />
            </div>
            <h1
              className={cn(
                'font-bold text-xl tracking-tight whitespace-nowrap transition-opacity duration-(--sidebar-duration) ease-(--sidebar-ease)',
                !isOpen &&
                  'opacity-0 duration-(--sidebar-fade-out) pointer-events-none',
              )}
            >
              <Trans>Rust-Srec</Trans>
            </h1>
          </Link>
        </Button>
        <Menu isOpen={isOpen} className="flex-1" />
      </div>
    </Sidebar>
  );
}
