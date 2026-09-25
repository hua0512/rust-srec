import { DanmuStatisticsObjectSchema } from '@/api/schemas';
import type { z } from 'zod';

type DanmuStatistics = z.infer<typeof DanmuStatisticsObjectSchema>;

const UNSET_DANMU_STATISTICS = Object.fromEntries(
  Object.keys(DanmuStatisticsObjectSchema.shape).map((key) => [key, undefined]),
) as DanmuStatistics;

/**
 * The form value a layer's danmu statistics start from: the stored settings, with every other key
 * present but `undefined`.
 *
 * The statistics inputs register fields inside this object, and react-hook-form writes each one
 * into the form values. Under a `null` object that value is `null`, which the schema rejects and
 * which blocks saving the whole form. Under an object missing the key it is a new `undefined` key,
 * and since react-hook-form compares key counts, the form reads as changed from the moment the card
 * renders, including straight after a save. Declaring every key up front avoids both.
 */
export function danmuStatisticsFormValue(
  value: DanmuStatistics | null | undefined,
): DanmuStatistics {
  return { ...UNSET_DANMU_STATISTICS, ...value };
}
