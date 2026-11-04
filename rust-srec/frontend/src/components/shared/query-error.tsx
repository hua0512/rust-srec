import type { ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
import { AlertCircle, RefreshCw } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';

interface QueryErrorProps {
  title: ReactNode;
  error: unknown;
  onRetry: () => void;
  className?: string;
}

/**
 * A failed fetch, said as such rather than as an empty list, with a way to try
 * again.
 */
export function QueryError({
  title,
  error,
  onRetry,
  className,
}: QueryErrorProps) {
  const message = error instanceof Error ? error.message : undefined;
  return (
    <Alert variant="destructive" className={className}>
      <AlertCircle className="h-4 w-4" />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription className="space-y-3">
        {message && <p>{message}</p>}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={onRetry}
          className="text-foreground"
        >
          <RefreshCw className="mr-2 h-4 w-4" />
          <Trans>Retry</Trans>
        </Button>
      </AlertDescription>
    </Alert>
  );
}
