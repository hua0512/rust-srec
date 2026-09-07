import { Trans } from '@lingui/react/macro';
import { AlertTriangle } from 'lucide-react';
import { Alert, AlertDescription } from '@/components/ui/alert';

interface PresetLookupStatusProps {
  loading: string[];
  failed: string[];
  missing: string[];
}

export function PresetLookupStatus({
  loading,
  failed,
  missing,
}: PresetLookupStatusProps) {
  if (!loading.length && !failed.length && !missing.length) return null;
  return (
    <Alert role="status" className="mb-3 break-words">
      <AlertTriangle className="h-4 w-4" />
      <AlertDescription className="space-y-1 max-h-24 overflow-y-auto">
        {loading.length > 0 && (
          <p>
            <Trans>Loading presets: {loading.join(', ')}</Trans>
          </p>
        )}
        {failed.length > 0 && (
          <p>
            <Trans>Could not load presets: {failed.join(', ')}</Trans>
          </p>
        )}
        {missing.length > 0 && (
          <p>
            <Trans>Missing presets: {missing.join(', ')}</Trans>
          </p>
        )}
        <p>
          <Trans>Delete-after-transform checks are incomplete.</Trans>
        </p>
      </AlertDescription>
    </Alert>
  );
}
