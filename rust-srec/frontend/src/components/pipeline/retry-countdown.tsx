import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

import { useNow } from '@/hooks/use-now';
import { formatLocalizedDuration } from '@/lib/date-utils';

/**
 * The time left until a scheduled automatic retry at `retryAt` (epoch
 * milliseconds). The retry starts shortly after its time, so once that has
 * passed it reads "Retrying shortly" until the job runs again.
 */
export function RetryCountdown({ retryAt }: { retryAt: number }) {
  const { i18n } = useLingui();
  const now = useNow();
  const waitSecs = Math.ceil((retryAt - now) / 1000);
  if (waitSecs > 0) {
    const wait = formatLocalizedDuration(waitSecs, i18n.locale);
    return <Trans>Retrying in {wait}</Trans>;
  }
  return <Trans>Retrying shortly</Trans>;
}
