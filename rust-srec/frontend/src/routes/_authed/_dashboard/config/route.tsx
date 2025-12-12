import { createFileRoute, Outlet, Link, useLocation, redirect } from '@tanstack/react-router';
import { Tabs, TabsList, TabsTrigger } from '../../../../components/ui/tabs';
import { Trans } from '@lingui/react/macro';

import { Settings } from 'lucide-react';

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

function ConfigLayout() {
  const { pathname } = useLocation();

  // Determine which tab is active based on the URL
  const currentTab = pathname.includes('/platforms') ? 'platforms' :
    pathname.includes('/templates') ? 'templates' :
      pathname.includes('/engines') ? 'engines' :
        pathname.includes('/theme') ? 'theme' :
          'global';

  return (
    <div className="min-h-screen space-y-6">
      <Tabs value={currentTab} className="space-y-0">
        {/* Header */}
        <div className="border-b border-border/40">
          <div className="w-full">
            {/* Title Row */}
            <div className="flex flex-col md:flex-row gap-4 items-start md:items-center justify-between p-4 md:px-8 pt-6">
              <div className="flex items-center gap-4">
                <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10 shadow-sm">
                  <Settings className="h-6 w-6 text-primary" />
                </div>
                <div>
                  <h1 className="text-xl font-semibold tracking-tight"><Trans>Settings</Trans></h1>
                  <p className="text-sm text-muted-foreground">
                    <Trans>Manage your application preferences and system configuration</Trans>
                  </p>
                </div>
              </div>
            </div>

            {/* Navigation Tabs */}
            <div className="px-4 md:px-8 pb-3 overflow-x-auto no-scrollbar">
              <TabsList className="h-auto p-0 bg-transparent gap-2 border-0 rounded-none w-auto justify-start inline-flex">
                <Link to="/config/global">
                  <TabsTrigger
                    value="global"
                    className="relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground data-[state=active]:shadow-sm text-muted-foreground hover:text-foreground hover:bg-muted"
                  >
                    <Trans>Global</Trans>
                  </TabsTrigger>
                </Link>
                <Link to="/config/platforms">
                  <TabsTrigger
                    value="platforms"
                    className="relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground data-[state=active]:shadow-sm text-muted-foreground hover:text-foreground hover:bg-muted"
                  >
                    <Trans>Platforms</Trans>
                  </TabsTrigger>
                </Link>
                <Link to="/config/templates">
                  <TabsTrigger
                    value="templates"
                    className="relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground data-[state=active]:shadow-sm text-muted-foreground hover:text-foreground hover:bg-muted"
                  >
                    <Trans>Templates</Trans>
                  </TabsTrigger>
                </Link>
                <Link to="/config/engines">
                  <TabsTrigger
                    value="engines"
                    className="relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground data-[state=active]:shadow-sm text-muted-foreground hover:text-foreground hover:bg-muted"
                  >
                    <Trans>Engines</Trans>
                  </TabsTrigger>
                </Link>
                <Link to="/config/theme">
                  <TabsTrigger
                    value="theme"
                    className="relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 data-[state=active]:bg-primary data-[state=active]:text-primary-foreground data-[state=active]:shadow-sm text-muted-foreground hover:text-foreground hover:bg-muted"
                  >
                    <Trans>Theme</Trans>
                  </TabsTrigger>
                </Link>
              </TabsList>
            </div>
          </div>
        </div>

        <div className="w-full px-4 md:px-8 py-8">
          <Outlet />
        </div>
      </Tabs>
    </div>
  );
}
