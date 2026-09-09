import { createFileRoute, Outlet, ScriptOnce } from '@tanstack/react-router';
import * as React from 'react';

import { AppSidebar } from '@/components/layout/app-sidebar';
import { SiteHeader } from '@/components/layout/site-header';
import { Footer } from '@/components/sidebar/footer';
import { Skeleton } from '@/components/ui/skeleton';
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar';
import { SidebarConfigProvider } from '@/contexts/sidebar-context';
import { useSidebarConfig } from '@/contexts/sidebar-context';
import {
  SIDEBAR_COOKIE_KEY,
  SIDEBAR_COOKIE_MAX_AGE_SECONDS,
  readSidebarCookie,
} from '@/lib/sidebar-cookie';

export const Route = createFileRoute('/_authed/_dashboard')({
  component: DashboardBaseLayout,
});

// ── inline script: reads cookie before first paint so the very first
//    React render on the client can pick up the correct value
//    synchronously (avoids FOUC). ──────────────────────────────────
const SIDEBAR_INIT_SCRIPT = `
(function(){
  try {
    var m = document.cookie.match(/(?:^|; )${SIDEBAR_COOKIE_KEY}=(true|false)/);
    if (m) document.documentElement.dataset.sidebarState = m[1];
  } catch(e) {}
})();
`;

/**
 * Synchronously read the stored state. The inline script above stashes it on
 * `<html>` during the SSR document parse; the cookie itself is the fallback for
 * the desktop build, which has no SSR document for that script to run in.
 */
function getClientSidebarState(): boolean | undefined {
  if (typeof document === 'undefined') return undefined;
  const v = document.documentElement.dataset.sidebarState;
  if (v === 'true') return true;
  if (v === 'false') return false;
  return readSidebarCookie(document.cookie);
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
  const { sidebar: ssrSidebar } = Route.useRouteContext();

  // First render: use the value the request middleware read from the cookie,
  // which is what the server rendered. On the client the inline <script> has
  // already stashed the same cookie value onto <html>, so the two agree and
  // hydration matches.
  const [sidebarOpen, _setSidebarOpen] = React.useState(
    () => getClientSidebarState() ?? ssrSidebar.open,
  );

  const setSidebarOpen = React.useCallback((value: boolean) => {
    _setSidebarOpen(value);
    try {
      document.cookie = `${SIDEBAR_COOKIE_KEY}=${value}; path=/; max-age=${SIDEBAR_COOKIE_MAX_AGE_SECONDS}`;
    } catch {
      // ignore
    }
  }, []);

  // One-time reconciliation: if there was no cookie but localStorage has
  // a preference, adopt it and write a cookie for future loads.
  React.useEffect(() => {
    if (readSidebarCookie(document.cookie) !== undefined) return;

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
          <React.Suspense fallback={<DashboardSkeleton />}>
            <Outlet />
          </React.Suspense>
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

function DashboardSkeleton() {
  return (
    <div className="space-y-6 animate-pulse">
      <div className="space-y-2">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-4 w-72" />
      </div>
      <div className="grid gap-4 md:gap-6 md:grid-cols-2 lg:grid-cols-4">
        {Array.from({ length: 4 }).map((_, i) => (
          <Skeleton key={i} className="h-32 rounded-2xl" />
        ))}
      </div>
      <Skeleton className="h-64 rounded-2xl" />
    </div>
  );
}
