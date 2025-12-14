'use client';
import { Menu } from '@/components/sidebar/menu';
import { SidebarToggle } from '@/components/sidebar/sidebar-toggle';
import { Button } from '@/components/ui/button';
import { useSidebar } from '@/store/sidebar';
import { useStore } from '@/hooks/use-store';
import { cn } from '@/lib/utils';
import { Link } from '@tanstack/react-router';

import { useShallow } from 'zustand/react/shallow';

export function Sidebar() {
  const sidebar = useStore(
    useSidebar,
    useShallow((state) => ({
      isOpen: state.isOpen || (state.settings.isHoverOpen && state.isHover),
      settings: state.settings,
      toggleOpen: state.toggleOpen,
      setIsHover: state.setIsHover,
    })),
  );
  if (!sidebar) return null;
  const { isOpen, toggleOpen, setIsHover, settings } = sidebar;
  return (
    <aside
      className={cn(
        'fixed top-0 left-0 z-20 h-screen -translate-x-full lg:translate-x-0 transition-[width] ease-in-out duration-300 bg-sidebar border-r border-border',
        !isOpen ? 'w-[90px]' : 'w-72',
        settings.disabled && 'hidden',
      )}
    >
      <SidebarToggle isOpen={isOpen} setIsOpen={toggleOpen} />
      <div
        onMouseEnter={() => setIsHover(true)}
        onMouseLeave={() => setIsHover(false)}
        className="relative h-full flex flex-col px-3 py-4 overflow-hidden shadow-md dark:shadow-zinc-800"
      >
        <Button
          className={cn(
            'transition-transform ease-in-out duration-300 mb-1',
            !isOpen ? 'translate-x-1' : 'translate-x-0',
          )}
          variant="link"
          asChild
        >
          <Link to="/dashboard" className="flex items-center gap-2">
            <div className="w-8 h-8 mr-1 bg-primary dark:bg-primary transition-colors [mask-image:url(/stream-rec-white.svg)] [mask-size:contain] [mask-repeat:no-repeat] [mask-position:center]" />
            <h1
              className={cn(
                'font-bold text-lg whitespace-nowrap transition-[transform,opacity,display] ease-in-out duration-300',
                !isOpen
                  ? '-translate-x-96 opacity-0 hidden'
                  : 'translate-x-0 opacity-100',
              )}
            >
              Rust-Srec
            </h1>
          </Link>
        </Button>
        <Menu isOpen={isOpen} className="flex-1" />
      </div>
    </aside>
  );
}
