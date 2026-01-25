import { createServerFn } from '@/server/createServerFn';
import { fetchBackend } from '../api';
import { HealthSchema } from '../../api/schemas';

export const getSystemHealth = createServerFn({ method: 'GET' }).handler(
  async () => {
    const json = await fetchBackend('/health');
    return HealthSchema.parse(json);
  },
);
