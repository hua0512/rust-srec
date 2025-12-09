import { createFileRoute, Outlet, Link, useLocation, redirect } from '@tanstack/react-router';
import { Tabs, TabsList, TabsTrigger } from '../../../../components/ui/tabs';
import { Trans } from '@lingui/react/macro';

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
    <div className="space-y-6">
      <Tabs value={currentTab} className="space-y-4">
        <TabsList>
          <Link to="/config/global">
            <TabsTrigger value="global"><Trans>Global</Trans></TabsTrigger>
          </Link>
          <Link to="/config/platforms">
            <TabsTrigger value="platforms"><Trans>Platforms</Trans></TabsTrigger>
          </Link>
          <Link to="/config/templates">
            <TabsTrigger value="templates"><Trans>Templates</Trans></TabsTrigger>
          </Link>
          <Link to="/config/engines">
            <TabsTrigger value="engines"><Trans>Engines</Trans></TabsTrigger>
          </Link>
          <Link to="/config/theme">
            <TabsTrigger value="theme"><Trans>Theme</Trans></TabsTrigger>
          </Link>
        </TabsList>
        <Outlet />
      </Tabs>
    </div>
  );
}
