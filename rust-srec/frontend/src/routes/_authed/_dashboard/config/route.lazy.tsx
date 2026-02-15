import {
  createLazyFileRoute,
  Outlet,
  Link,
  useLocation,
} from '@tanstack/react-router';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import {
  Settings,
  Globe,
  LayoutTemplate,
  Cpu,
  Palette,
  Share2,
  Archive,
  Terminal,
  Languages,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { motion } from 'motion/react';
import { Separator } from '@/components/ui/separator';

export const Route = createLazyFileRoute('/_authed/_dashboard/config')({
  component: ConfigLayout,
});

interface SidebarItem {
  title: string;
  href: string;
  icon: React.ElementType;
  description: string;
}

function ConfigLayout() {
  const { pathname } = useLocation();
  const { i18n } = useLingui();

  const sidebarItems: SidebarItem[] = [
    {
      title: i18n._(msg`Global`),
      href: '/config/global',
      icon: Globe,
      description: i18n._(msg`System-wide preferences`),
    },
    {
      title: i18n._(msg`Platforms`),
      href: '/config/platforms',
      icon: Share2,
      description: i18n._(msg`Streaming services`),
    },
    {
      title: i18n._(msg`Templates`),
      href: '/config/templates',
      icon: LayoutTemplate,
      description: i18n._(msg`Job configurations`),
    },
    {
      title: i18n._(msg`Engines`),
      href: '/config/engines',
      icon: Cpu,
      description: i18n._(msg`Processing nodes`),
    },
    {
      title: i18n._(msg`Logging`),
      href: '/config/logging',
      icon: Terminal,
      description: i18n._(msg`Log levels & modules`),
    },
    {
      title: i18n._(msg`Theme`),
      href: '/config/theme',
      icon: Palette,
      description: i18n._(msg`Appearance & style`),
    },
    {
      title: i18n._(msg`Backup & Restore`),
      href: '/config/backup',
      icon: Archive,
      description: i18n._(msg`Backup & Restore`),
    },
    {
      title: i18n._(msg`Language`),
      href: '/config/language',
      icon: Languages,
      description: i18n._(msg`Choose your display language`),
    },
  ];

  return (
    <div className="flex h-full flex-col space-y-6 lg:flex-row lg:space-x-8 lg:space-y-0 w-full min-h-[calc(100vh-4rem)] overflow-x-hidden lg:overflow-x-visible">
      <aside className="w-full lg:w-64 shrink-0 self-start lg:sticky top-24 flex flex-col gap-4 sm:gap-6 lg:px-0 min-w-0">
        <div className="flex items-center gap-3 px-3 lg:px-2">
          <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10 shadow-sm">
            <Settings className="h-6 w-6 text-primary" />
          </div>
          <div>
            <h1 className="text-xl font-semibold tracking-tight">
              <Trans>Settings</Trans>
            </h1>
          </div>
        </div>

        <Separator className="hidden lg:block opacity-50" />

        <nav className="sticky top-[56px] z-20 lg:relative lg:top-0 -mx-3 px-3 lg:mx-0 lg:px-0 bg-background/95 lg:bg-transparent backdrop-blur-md lg:backdrop-blur-0 border-b lg:border-b-0 border-border/50 py-3 lg:py-0 flex w-full overflow-x-auto lg:overflow-visible no-scrollbar">
          <div className="flex gap-2 min-w-0 lg:flex-col lg:w-full lg:gap-2 px-3 lg:px-0">
            {sidebarItems.map((item) => {
              const isActive = pathname.includes(item.href);
              return (
                <Link
                  key={item.href}
                  to={item.href}
                  className={cn(
                    'group flex shrink-0 lg:shrink items-center gap-2 rounded-xl px-3 py-2 lg:px-4 lg:py-3 text-sm font-medium transition-all hover:bg-accent hover:text-accent-foreground whitespace-nowrap',
                    isActive
                      ? 'bg-primary/10 text-primary'
                      : 'text-muted-foreground',
                  )}
                >
                  <item.icon
                    className={cn(
                      'h-4 w-4 transition-colors shrink-0',
                      isActive
                        ? 'text-primary'
                        : 'text-muted-foreground group-hover:text-foreground',
                    )}
                  />
                  <span
                    className={cn(
                      'font-semibold',
                      isActive
                        ? 'text-primary'
                        : 'text-muted-foreground group-hover:text-foreground',
                    )}
                  >
                    {item.title}
                  </span>
                </Link>
              );
            })}
          </div>
        </nav>
      </aside>

      <div className="flex-1 min-w-0">
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
