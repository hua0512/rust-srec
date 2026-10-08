import { queryOptions } from '@tanstack/react-query';
import { listEngines } from '@/server/functions';

/** The engine list, shared by every page that reads it under one key. */
export const enginesQueryOptions = queryOptions({
  queryKey: ['engines'],
  queryFn: () => listEngines(),
});
