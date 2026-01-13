import { useEffect } from 'react';
import { ThemeProvider as NextThemesProvider } from 'next-themes';
import { useThemeStore, applyTheme } from '../stores/theme-store';

export function ThemeProvider({
  children,
  ...props
}: React.ComponentProps<typeof NextThemesProvider>) {
  const themeState = useThemeStore();

  // Apply theme-specific side effects after the component mounts on the client
  // to avoid hydration mismatches (direct DOM mutation during render).
  useEffect(() => {
    applyTheme(themeState);
  }, [themeState]);

  return (
    <NextThemesProvider
      attribute="class"
      defaultTheme="system"
      enableSystem
      {...props}
    >
      {children}
    </NextThemesProvider>
  );
}

export { useTheme } from 'next-themes';
