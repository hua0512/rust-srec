import { Trans } from '@lingui/react/macro';
import { useQuery } from '@tanstack/react-query';
import { getSystemHealth } from '@/server/functions';

const UI_BUILD =
  typeof import.meta.env.VITE_UI_BUILD === 'string' &&
  import.meta.env.VITE_UI_BUILD.length > 0
    ? import.meta.env.VITE_UI_BUILD
    : 'dev';

export function Footer() {
  const { data: health } = useQuery({
    queryKey: ['health', 'version'],
    queryFn: () => getSystemHealth(),
    staleTime: 5 * 60 * 1000,
    retry: false,
    refetchOnWindowFocus: false,
  });

  const backendVersion = health?.version ?? null;

  return (
    <div className="z-20 w-full bg-background/95 shadow backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="mx-4 md:mx-8 flex h-14 items-center">
        <p className="text-xs md:text-sm leading-loose text-muted-foreground text-left">
          <Trans>
            Powered by{' '}
            <a
              href="https://github.com/hua0512/rust-srec"
              target="_blank"
              rel="noopener noreferrer"
              className="font-medium underline underline-offset-4"
            >
              Rust-srec
            </a>
            .
          </Trans>{' '}
          Backend: {backendVersion ?? '-'}{' '}
          <span className="mx-1 opacity-60 text-[10px]">|</span> UI: {UI_BUILD}
        </p>
      </div>
    </div>
  );
}
