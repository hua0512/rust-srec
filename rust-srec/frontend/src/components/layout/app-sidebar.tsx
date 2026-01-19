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

      <div
        className={cn(
          'relative h-full flex flex-col py-4 overflow-hidden',
          isOpen ? 'px-3' : 'px-0',
        )}
      >
        <Button
          className={cn(
            'w-full transition-all ease-in-out duration-300 mb-6 bg-transparent hover:bg-transparent',
            !isOpen ? 'justify-center' : 'justify-start px-4',
          )}
          variant="link"
          asChild
        >
          <Link
            to="/dashboard"
            className={cn('flex items-center', isOpen ? 'gap-4' : 'gap-0')}
          >
            <div className="w-10 h-10 bg-primary rounded-xl flex items-center justify-center shrink-0 shadow-lg shadow-primary/20 transition-all duration-300">
              <div className="w-6 h-6 bg-primary-foreground [mask-image:url(/stream-rec-white.svg)] [mask-size:contain] [mask-repeat:no-repeat] [mask-position:center]" />
            </div>
            <h1
              className={cn(
                'font-bold text-xl tracking-tight whitespace-nowrap transition-all ease-in-out duration-300',
                !isOpen
                  ? 'opacity-0 w-0 pointer-events-none'
                  : 'opacity-100 translate-x-0',
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
