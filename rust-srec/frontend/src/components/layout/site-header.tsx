import React from 'react';
import { Link, useLocation } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { MenuIcon } from 'lucide-react';

import { ConnectionStatusIndicator } from '@/components/connection-status-indicator';
import { LanguageSwitcher } from '@/components/language-switcher';
import { ModeToggle } from '@/components/sidebar/mode-toggle';
import { Button } from '@/components/ui/button';
import { useSidebarConfig } from '@/hooks/use-sidebar-config';
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from '@/components/ui/breadcrumb';
import { SidebarTrigger, useSidebar } from '@/components/ui/sidebar';

export function SiteHeader() {
  const { config } = useSidebarConfig();
  const { isMobile, toggleSidebar } = useSidebar();
  const location = useLocation();
  const pathSegments = location.pathname.split('/').filter(Boolean);

  const BREADCRUMB_NAME_MAP: Record<string, React.ReactNode> = {
    dashboard: <Trans>Dashboard</Trans>,
    sessions: <Trans>Sessions</Trans>,
    settings: <Trans>Settings</Trans>,
    streamers: <Trans>Streamers</Trans>,
    config: <Trans>Config</Trans>,
    logs: <Trans>Logs</Trans>,
    'change-password': <Trans>Change Password</Trans>,
    users: <Trans>Users</Trans>,
    pipeline: <Trans>Pipeline</Trans>,
    notifications: <Trans>Notifications</Trans>,
    events: <Trans>Events</Trans>,
    player: <Trans>Player</Trans>,
    system: <Trans>System</Trans>,
    backup: <Trans>Backup</Trans>,
    engines: <Trans>Engines</Trans>,
    platforms: <Trans>Platforms</Trans>,
    templates: <Trans>Templates</Trans>,
    theme: <Trans>Theme</Trans>,
    global: <Trans>Global</Trans>,
    executions: <Trans>Executions</Trans>,
    jobs: <Trans>Jobs</Trans>,
    outputs: <Trans>Outputs</Trans>,
    presets: <Trans>Presets</Trans>,
    workflows: <Trans>Workflows</Trans>,
    health: <Trans>Health</Trans>,
    new: <Trans>New</Trans>,
    edit: <Trans>Edit</Trans>,
  };

  return (
    <header className="sticky top-0 z-15 w-full bg-background/95 shadow backdrop-blur supports-[backdrop-filter]:bg-background/60 dark:shadow-secondary">
      <div className="px-4 sm:px-8 flex h-14 items-center">
        <div className="flex items-center space-x-4 lg:space-x-0">
          {config.collapsible !== 'none' ? (
            isMobile ? (
              <Button
                className="h-8"
                variant="outline"
                size="icon"
                onClick={toggleSidebar}
              >
                <MenuIcon size={20} />
              </Button>
            ) : config.collapsible === 'offcanvas' ? (
              <SidebarTrigger variant="outline" className="-ml-1 size-8" />
            ) : null
          ) : null}
          <Breadcrumb className="hidden sm:block">
            <BreadcrumbList>
              <BreadcrumbItem>
                <BreadcrumbLink asChild>
                  <Link to="/dashboard">
                    <Trans>Home</Trans>
                  </Link>
                </BreadcrumbLink>
              </BreadcrumbItem>
              {pathSegments.map((segment, index) => {
                const isLast = index === pathSegments.length - 1;
                const href = `/${pathSegments.slice(0, index + 1).join('/')}`;
                const breadcrumbName = BREADCRUMB_NAME_MAP[segment] || segment;

                return (
                  <React.Fragment key={href}>
                    <BreadcrumbSeparator />
                    <BreadcrumbItem>
                      {isLast ? (
                        <BreadcrumbPage className="capitalize">
                          {breadcrumbName}
                        </BreadcrumbPage>
                      ) : (
                        <BreadcrumbLink asChild className="capitalize">
                          <Link to={href}>{breadcrumbName}</Link>
                        </BreadcrumbLink>
                      )}
                    </BreadcrumbItem>
                  </React.Fragment>
                );
              })}
            </BreadcrumbList>
          </Breadcrumb>
        </div>
        <div className="flex flex-1 items-center justify-end space-x-4">
          <ConnectionStatusIndicator />
          <LanguageSwitcher />
          <ModeToggle />
        </div>
      </div>
    </header>
  );
}
