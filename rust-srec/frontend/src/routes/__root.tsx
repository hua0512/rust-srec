import { HeadContent, Scripts, createRootRoute } from '@tanstack/react-router'
import { TanStackRouterDevtoolsPanel } from '@tanstack/react-router-devtools'
import { TanStackDevtools } from '@tanstack/react-devtools'



import appCss from '../styles.css?url'

export const Route = createRootRoute({
  head: () => ({
    meta: [
      {
        charSet: 'utf-8',
      },
      {
        name: 'viewport',
        content: 'width=device-width, initial-scale=1',
      },
      {
        title: 'TanStack Start Starter',
      },
    ],
    links: [
      {
        rel: 'stylesheet',
        href: appCss,
      },
      {
        rel: 'preconnect',
        href: 'https://fonts.googleapis.com',
      },
      {
        rel: 'preconnect',
        href: 'https://fonts.gstatic.com',
        crossOrigin: 'anonymous',
      },
      {
        rel: 'stylesheet',
        href: 'https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap',
      },
    ],
  }),

  shellComponent: RootDocument,
  notFoundComponent: () => {
    return (
      <div className="flex flex-col items-center justify-center p-12 text-center text-muted-foreground w-full h-[50vh]">
        <h1 className="text-4xl font-bold mb-4">404</h1>
        <p className="text-xl">Page not found</p>
      </div>
    )
  }
})

import { I18nProvider } from "@lingui/react"
import { i18n } from "../i18n"
import { ThemeProvider } from "../components/theme-provider"
import { Toaster } from "../components/ui/sonner"

import { QueryClient, QueryClientProvider } from '@tanstack/react-query'

const queryClient = new QueryClient()

function RootDocument({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <HeadContent />
      </head>
      <body suppressHydrationWarning>
        <QueryClientProvider client={queryClient}>
          <I18nProvider i18n={i18n}>
            <ThemeProvider defaultTheme="system" storageKey="vite-ui-theme">
              {children}
              <Toaster />
              <TanStackDevtools
                config={{
                  position: 'bottom-right',
                }}
                plugins={[
                  {
                    name: 'Tanstack Router',
                    render: <TanStackRouterDevtoolsPanel />,
                  },
                ]}
              />
              <Scripts />
            </ThemeProvider>
          </I18nProvider>
        </QueryClientProvider>
      </body>
    </html>
  )
}
