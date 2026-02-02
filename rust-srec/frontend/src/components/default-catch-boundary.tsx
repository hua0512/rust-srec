import {
  ErrorComponent,
  Link,
  rootRouteId,
  useMatch,
  useRouter,
} from '@tanstack/react-router';
import type { ErrorComponentProps } from '@tanstack/react-router';
import { Button } from '@/components/ui/button';
import { AlertCircle, RefreshCcw } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

export function DefaultCatchBoundary({ error }: ErrorComponentProps) {
  const router = useRouter();
  const isRoot = useMatch({
    strict: false,
    select: (state) => state.id === rootRouteId,
  });

  console.error(error);

  return (
    <div className="flex flex-col items-center justify-center min-h-[50vh] space-y-4 p-4 text-center">
      <AlertCircle className="h-24 w-24 text-destructive opacity-80" />
      <div className="space-y-2">
        <h2 className="text-3xl font-bold tracking-tight">
          <Trans>Something went wrong!</Trans>
        </h2>
        <div className="text-muted-foreground max-w-md mx-auto">
          <ErrorComponent error={error} />
        </div>
      </div>

      <div className="flex items-center gap-2">
        <Button
          onClick={() => {
            void router.invalidate();
          }}
          variant="default"
        >
          <RefreshCcw className="mr-2 h-4 w-4" />
          <Trans>Try Again</Trans>
        </Button>
        {isRoot ? (
          <Button variant="outline" asChild>
            <Link to="/">
              <Trans>Home</Trans>
            </Link>
          </Button>
        ) : (
          <Button
            variant="outline"
            onClick={(e) => {
              e.preventDefault();
              window.history.back();
            }}
          >
            <Trans>Go Back</Trans>
          </Button>
        )}
      </div>
    </div>
  );
}
