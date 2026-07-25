import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { extractMetadata } from '@/server/functions';

/** Matches the debounce used by `components/shared/search-input.tsx`. */
const DEBOUNCE_MS = 400;

/**
 * Detect which platform a streamer URL belongs to.
 *
 * The backend derives `platform_config_id` from the URL on create and update, so detection has to
 * come from the same place rather than a client-side URL table that could disagree with it. This
 * calls `/streamers/extract-metadata`, which runs the same resolution the write path does —
 * including the `streamlink` fallback for URLs no built-in platform claims.
 *
 * The URL is debounced so typing doesn't fire a request per keystroke, and results are cached per
 * URL by react-query so revisiting one costs nothing.
 */
export function usePlatformDetection(url: string | undefined) {
  const [debouncedUrl, setDebouncedUrl] = useState(url ?? '');

  useEffect(() => {
    const timer = setTimeout(() => setDebouncedUrl(url ?? ''), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [url]);

  // `extract-metadata` rejects anything that isn't a well-formed http(s) URL, so skip the
  // round-trip until the field could plausibly parse.
  const enabled = /^https?:\/\/\S+$/.test(debouncedUrl.trim());

  const { data, isFetching, isError } = useQuery({
    queryKey: ['platform-detection', debouncedUrl],
    queryFn: () => extractMetadata({ data: debouncedUrl.trim() }),
    enabled,
    retry: false,
    staleTime: 5 * 60 * 1000,
  });

  const isPending = enabled && (isFetching || (!data && !isError));

  return {
    /** Detected platform name, e.g. `Bilibili`. Null while pending or unsupported. */
    platform: data?.platform ?? null,
    isDetecting: isPending,
    /** The URL parses but no platform claims it, so saving would be rejected. */
    isUnsupported: enabled && !isPending && (isError || !data?.platform),
  };
}
