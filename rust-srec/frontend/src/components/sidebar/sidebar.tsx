import { Menu } from '@/components/sidebar/menu';
import { SidebarToggle } from '@/components/sidebar/sidebar-toggle';
import { Button } from '@/components/ui/button';
import { useSidebar } from '@/store/sidebar';
import { useStore } from '@/hooks/use-store';
import { cn } from '@/lib/utils';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';

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
        'fixed top-0 left-0 z-20 h-screen -translate-x-full lg:translate-x-0 transition-[width] ease-in-out duration-300 bg-sidebar/80 backdrop-blur-xl border-r border-border/50',
        !isOpen ? 'w-[90px]' : 'w-72',
        settings.disabled && 'hidden',
      )}
    >
      <SidebarToggle isOpen={isOpen} setIsOpen={toggleOpen} />
      <div
        onMouseEnter={() => setIsHover(true)}
        onMouseLeave={() => setIsHover(false)}
        className={cn(
          'relative h-full flex flex-col py-4 overflow-hidden',
          !isOpen ? 'px-0' : 'px-3',
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
            <div className="w-10 h-10 bg-primary rounded-xl flex items-center justify-center shrink-0 shadow-lg shadow-primary/20 transition-all duration-300 group-hover:scale-105">
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
    </aside>
  );
}
