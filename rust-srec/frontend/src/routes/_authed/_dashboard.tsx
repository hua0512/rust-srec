import { ClientOnly, createFileRoute, Outlet } from '@tanstack/react-router';
import * as React from 'react';

import { AppSidebar } from '@/components/layout/app-sidebar';
import { SiteHeader } from '@/components/layout/site-header';
import { Footer } from '@/components/sidebar/footer';
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar';
import { SidebarConfigProvider } from '@/contexts/sidebar-context';
import { useSidebarConfig } from '@/hooks/use-sidebar-config';

export const Route = createFileRoute('/_authed/_dashboard')({
  component: DashboardBaseLayout,
});

function DashboardBaseLayout() {
  return (
    <ClientOnly>
      <SidebarConfigProvider>
        <DashboardLayout />
      </SidebarConfigProvider>
    </ClientOnly>
  );
}

function DashboardLayout() {
  const { config } = useSidebarConfig();
  // const { isOpen: themeCustomizerOpen, setIsOpen: setThemeCustomizerOpen } =
  //   useThemeCustomizer();
  const [defaultSidebarOpen] = React.useState<boolean>(() => {
    if (typeof window === 'undefined') return true;

    try {
      const cookieValue = document.cookie
        .split('; ')
        .find((row) => row.startsWith('sidebar_state='))
        ?.split('=')[1];
      if (cookieValue === 'true') return true;
      if (cookieValue === 'false') return false;
    } catch {
      // ignore
    }

    try {
      const raw = localStorage.getItem('sidebar');
      if (!raw) return true;
      const parsed = JSON.parse(raw) as { state?: { isOpen?: boolean } };
      if (typeof parsed?.state?.isOpen === 'boolean') {
        try {
          document.cookie = `sidebar_state=${parsed.state.isOpen}; path=/; max-age=${
            60 * 60 * 24 * 7
          }`;
        } catch {
          // ignore
        }
        return parsed.state.isOpen;
      }
    } catch {
      // ignore
    }

    return true;
  });

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
      defaultOpen={defaultSidebarOpen}
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

      {/* <ThemeCustomizerTrigger onClick={() => setThemeCustomizerOpen(true)} /> */}
      {/* <ThemeCustomizer
        open={themeCustomizerOpen}
        onOpenChange={setThemeCustomizerOpen}
      /> */}
    </SidebarProvider>
  );
}
