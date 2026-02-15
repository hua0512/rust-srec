import { createFileRoute, Outlet, ScriptOnce } from '@tanstack/react-router';
import * as React from 'react';

import { AppSidebar } from '@/components/layout/app-sidebar';
import { SiteHeader } from '@/components/layout/site-header';
import { Footer } from '@/components/sidebar/footer';
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar';
import { SidebarConfigProvider } from '@/contexts/sidebar-context';
import { useSidebarConfig } from '@/hooks/use-sidebar-config';
import { createServerFn } from '@/server/createServerFn';

// ── server function: read sidebar cookie during SSR ──────────────
const getSidebarCookie = createServerFn({ method: 'GET' }).handler(async () => {
  try {
    const { getRequestHeader } = await import('@tanstack/react-start/server');
    const raw = getRequestHeader('cookie') ?? '';
    const match = raw.match(/(?:^|; )sidebar_state=(true|false)/);
    if (match) return match[1] === 'true';
  } catch {
    // outside request context – default to expanded
  }
  return true;
});

export const Route = createFileRoute('/_authed/_dashboard')({
  beforeLoad: async () => {
    const sidebarOpen = await getSidebarCookie();
    return { sidebarOpen };
  },
  component: DashboardBaseLayout,
});

// ── inline script: reads cookie before first paint so the very first
//    React render on the client can pick up the correct value
//    synchronously (avoids FOUC). ──────────────────────────────────
const SIDEBAR_INIT_SCRIPT = `
(function(){
  try {
    var m = document.cookie.match(/(?:^|; )sidebar_state=(true|false)/);
    if (m) document.documentElement.dataset.sidebarState = m[1];
  } catch(e) {}
})();
`;

/** Synchronously read the value stashed by the inline script. */
function getClientSidebarState(): boolean | undefined {
  if (typeof document === 'undefined') return undefined;
  const v = document.documentElement.dataset.sidebarState;
  if (v === 'true') return true;
  if (v === 'false') return false;
  return undefined;
}

function DashboardBaseLayout() {
  return (
    <SidebarConfigProvider>
      <ScriptOnce>{SIDEBAR_INIT_SCRIPT}</ScriptOnce>
      <DashboardLayout />
    </SidebarConfigProvider>
  );
}

function DashboardLayout() {
  const { config } = useSidebarConfig();
  const { sidebarOpen: ssrSidebarOpen } = Route.useRouteContext();

  // First render: use the server-provided value (which read the cookie).
  // On the client the inline <script> already stashed the same cookie
  // value onto <html>, so getClientSidebarState() agrees with the server.
  const [sidebarOpen, _setSidebarOpen] = React.useState(
    () => getClientSidebarState() ?? ssrSidebarOpen,
  );

  const setSidebarOpen = React.useCallback((value: boolean) => {
    _setSidebarOpen(value);
    try {
      document.cookie = `sidebar_state=${value}; path=/; max-age=${60 * 60 * 24 * 7}`;
    } catch {
      // ignore
    }
  }, []);

  // One-time reconciliation: if there was no cookie but localStorage has
  // a preference, adopt it and write a cookie for future loads.
  React.useEffect(() => {
    if (document.documentElement.dataset.sidebarState) return;

    try {
      const raw = localStorage.getItem('sidebar');
      if (!raw) return;
      const parsed = JSON.parse(raw) as { state?: { isOpen?: boolean } };
      if (typeof parsed?.state?.isOpen === 'boolean') {
        setSidebarOpen(parsed.state.isOpen);
      }
    } catch {
      // ignore
    }
  }, [setSidebarOpen]);

  const sidebar = (
    <AppSidebar
      variant={config.variant}
      collapsible={config.collapsible}
      side={config.side}
    />
  );

  const main = (
    <SidebarInset>
      <SiteHeader />
      <div className="flex flex-1 flex-col">
        <div className="w-full pt-8 pb-8 px-3 sm:px-8">
          <Outlet />
        </div>
        <Footer />
      </div>
    </SidebarInset>
  );

  return (
    <SidebarProvider
      open={sidebarOpen}
      onOpenChange={setSidebarOpen}
      style={
        {
          '--sidebar-width': '18rem',
          '--sidebar-width-icon': '90px',
        } as React.CSSProperties
      }
      className={config.collapsible === 'none' ? 'sidebar-none-mode' : ''}
    >
      {config.side === 'left' ? (
        <>
          {sidebar}
          {main}
        </>
      ) : (
        <>
          {main}
          {sidebar}
        </>
      )}
    </SidebarProvider>
  );
}
