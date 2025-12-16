import {
  createFileRoute,
  Outlet,
  Link,
  useLocation,
  redirect,
} from '@tanstack/react-router';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import {
  Settings,
  Globe,
  LayoutTemplate,
  Cpu,
  Palette,
  Share2,
  Archive,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { motion } from 'motion/react';
import { Separator } from '@/components/ui/separator';

export const Route = createFileRoute('/_authed/_dashboard/config')({
  component: ConfigLayout,
  beforeLoad: ({ location }) => {
    if (location.pathname === '/config') {
      throw redirect({
        to: '/config/global',
      });
    }
  },
});

interface SidebarItem {
  title: string;
  href: string;
  icon: React.ElementType;
  description: string;
}

function ConfigLayout() {
  const { pathname } = useLocation();

  const sidebarItems: SidebarItem[] = [
    {
      title: t`Global`,
      href: '/config/global',
      icon: Globe,
      description: t`System-wide preferences`,
    },
    {
      title: t`Platforms`,
      href: '/config/platforms',
      icon: Share2,
      description: t`Streaming services`,
    },
    {
      title: t`Templates`,
      href: '/config/templates',
      icon: LayoutTemplate,
      description: t`Job configurations`,
    },
    {
      title: t`Engines`,
      href: '/config/engines',
      icon: Cpu,
      description: t`Processing nodes`,
    },
    {
      title: t`Theme`,
      href: '/config/theme',
      icon: Palette,
      description: t`Appearance & style`,
    },
    {
      title: t`Backup & Restore`,
      href: '/config/backup',
      icon: Archive,
      description: t`Backup & Restore`,
    },
  ];

  return (
    <div className="flex h-full flex-col space-y-8 lg:flex-row lg:space-x-8 lg:space-y-0 w-full min-h-[calc(100vh-4rem)]">
      <aside className="lg:w-1/5 xl:w-1/6 self-start sticky top-24">
        <div className="flex flex-col gap-6">
          <div className="flex items-center gap-3 px-2">
            <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10 shadow-sm">
              <Settings className="h-6 w-6 text-primary" />
            </div>
            <div>
              <h1 className="text-xl font-semibold tracking-tight">
                <Trans>Settings</Trans>
              </h1>
            </div>
          </div>

          <Separator className="opacity-50" />

          <nav className="flex space-x-2 lg:flex-col lg:space-x-0 lg:space-y-2 overflow-x-auto lg:overflow-visible pb-2 lg:pb-0 no-scrollbar">
            {sidebarItems.map((item) => {
              const isActive = pathname.includes(item.href);
              return (
                <Link
                  key={item.href}
                  to={item.href}
                  className={cn(
                    'group flex min-w-[180px] lg:min-w-0 flex-col gap-1 rounded-xl px-4 py-3 text-sm font-medium transition-all hover:bg-accent',
                    isActive
                      ? 'bg-accent/80 text-accent-foreground'
                      : 'text-muted-foreground',
                  )}
                >
                  <div className="flex items-center gap-3">
                    <item.icon
                      className={cn(
                        'h-4 w-4 transition-colors',
                        isActive
                          ? 'text-primary'
                          : 'text-muted-foreground group-hover:text-foreground',
                      )}
                    />
                    <span
                      className={cn(
                        'font-semibold',
                        isActive
                          ? 'text-foreground'
                          : 'text-muted-foreground group-hover:text-foreground',
                      )}
                    >
                      {item.title}
                    </span>
                  </div>
                  {/* <p className="text-xs text-muted-foreground/60 pl-7 hidden lg:block line-clamp-1">
                    <Trans>{item.description}</Trans>
                  </p> */}
                </Link>
              );
            })}
          </nav>
        </div>
      </aside>

      <div className="flex-1">
        <motion.div
          initial={{ opacity: 0, x: 20 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ duration: 0.4, ease: 'easeOut' }}
          className="pb-20"
        >
          <Outlet />
        </motion.div>
      </div>
    </div>
  );
}
