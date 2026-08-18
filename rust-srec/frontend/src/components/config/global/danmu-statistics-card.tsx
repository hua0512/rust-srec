import { memo } from 'react';
import { useFormContext } from 'react-hook-form';
import { DanmuStatisticsCard as SharedDanmuStatisticsCard } from '@/components/config/shared/danmu-statistics-card';

/**
 * Global-layer wrapper for the shared danmu statistics card.
 *
 * The global config page renders a list of standalone cards rather than mounting
 * `SharedConfigEditor`, so the shared card — which takes the form as a prop —
 * gets it from context here. No `basePath`: these are the base values every other
 * layer inherits, not an override.
 */
export const GlobalDanmuStatisticsCard = memo(() => {
  const form = useFormContext();
  return <SharedDanmuStatisticsCard form={form} />;
});

GlobalDanmuStatisticsCard.displayName = 'GlobalDanmuStatisticsCard';
