import { useEffect, useMemo, useRef } from 'react';
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query';
import { queryKeys } from '@/api/query-keys';
import { getPipelineJobLogs } from '@/server/functions/pipeline';
import {
  newestLogPageOffset,
  replaceNewestLogPage,
  sameLogRows,
  type JobLogPages,
} from '@/lib/job-log-pages';

/** Matches the backend's page cap for `/pipeline/jobs/{id}/logs`. */
const LOG_PAGE_SIZE = 1000;

/**
 * A job's log as pages read on demand, kept current while the job runs and
 * read once more when it finishes. `enabled` holds the reads back until the
 * job itself has loaded.
 */
export function useJobLogs({
  jobId,
  enabled,
  isJobActive,
  isJobTerminal,
}: {
  jobId: string;
  enabled: boolean;
  isJobActive: boolean;
  isJobTerminal: boolean;
}) {
  const queryClient = useQueryClient();

  const logsQueryKey = useMemo(
    () => queryKeys.pipeline.jobs.logs(jobId),
    [jobId],
  );

  // The closing log rows are written as the job finishes, after the last read
  // below. Read the newest page once more for a job that finishes while being
  // watched (every earlier page is final, see `newestLogPageOffset`); one that
  // was already finished when the page opened has nothing to catch up on and
  // is left alone.
  const sawRunningJobRef = useRef(false);
  useEffect(() => {
    if (isJobActive) {
      sawRunningJobRef.current = true;
      return;
    }
    if (!isJobTerminal || !sawRunningJobRef.current) return;
    sawRunningJobRef.current = false;
    const offset = newestLogPageOffset(
      queryClient.getQueryData<JobLogPages>(logsQueryKey),
    );
    if (offset === undefined) return;
    getPipelineJobLogs({
      data: { id: jobId, limit: LOG_PAGE_SIZE, offset },
    }).then(
      (page) => {
        const prev = queryClient.getQueryData<JobLogPages>(logsQueryKey);
        const next = replaceNewestLogPage(prev, page);
        const held = prev?.pages.at(-1)?.items ?? [];
        // A capped log keeps its count while its rows move on, so the read
        // is refused even though it differs; every page has shifted then,
        // and only a full re-read lines them up.
        if (
          next === prev &&
          !sameLogRows(
            held,
            page.items,
            0,
            Math.max(held.length, page.items.length),
          )
        ) {
          void queryClient.invalidateQueries({ queryKey: logsQueryKey });
          return;
        }
        queryClient.setQueryData<JobLogPages>(logsQueryKey, next);
      },
      () => {}, // The already-loaded rows stay on screen
    );
  }, [isJobActive, isJobTerminal, jobId, logsQueryKey, queryClient]);

  // Fetch logs separately with infinite scroll. Deliberately not polled: a
  // reader who has scrolled back through a long job holds several pages, and
  // refreshing all of them on a timer re-reads thousands of rows that cannot
  // have changed. The tail query below keeps the live end current instead.
  const logsQuery = useInfiniteQuery({
    queryKey: logsQueryKey,
    queryFn: ({ pageParam }) =>
      getPipelineJobLogs({
        data: { id: jobId, limit: LOG_PAGE_SIZE, offset: pageParam },
      }),
    initialPageParam: 0,
    getNextPageParam: (lastPage) => {
      const nextOffset = lastPage.offset + lastPage.limit;
      return nextOffset < lastPage.total ? nextOffset : undefined;
    },
    enabled,
    // Log rows are only ever appended, so a page that came back full is final.
    // Nothing here has to be re-read on a focus or a reconnect; new rows arrive
    // through the tail query and the closing read above.
    staleTime: Infinity,
  });

  // One request per tick, always for the page a running job is still appending
  // to. Writing it back into the pages above both shows the new rows and
  // refreshes the total that decides whether another page follows.
  const tailOffset = newestLogPageOffset(logsQuery.data);
  const { data: logTailPage } = useQuery({
    queryKey: queryKeys.pipeline.jobs.logTail(jobId, tailOffset),
    queryFn: () =>
      getPipelineJobLogs({
        data: { id: jobId, limit: LOG_PAGE_SIZE, offset: tailOffset ?? 0 },
      }),
    enabled: isJobActive && tailOffset !== undefined,
    refetchInterval: 2000,
    throwOnError: false, // The already-loaded rows stay on screen
  });

  useEffect(() => {
    if (!logTailPage) return;
    queryClient.setQueryData<JobLogPages>(logsQueryKey, (prev) =>
      replaceNewestLogPage(prev, logTailPage),
    );
  }, [logTailPage, logsQueryKey, queryClient]);

  return logsQuery;
}
