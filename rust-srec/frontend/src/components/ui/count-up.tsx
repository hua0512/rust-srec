import { useEffect, useRef } from 'react';
import { animate } from 'motion';

interface CountUpProps {
  value: number | bigint;
  formatter?: (value: number | bigint) => string;
  className?: string;
}

// Live figures arrive irregularly: progress is applied in batches, and some
// recordings report only once per finished chunk of the stream. Each change is
// tweened linearly over a running average of the gap between changes, stretched
// so the tween is usually still running when the next value arrives. The number
// then moves at a steady rate instead of racing to each value and pausing. The
// bounds keep long gaps (a hidden tab, a stalled stream) from producing a tween
// that lags far behind.
const MIN_GAP_SECS = 0.25;
const MAX_GAP_SECS = 5;
const FIRST_TWEEN_SECS = 0.8;
const GAP_SMOOTHING = 0.3;
const TWEEN_STRETCH = 1.5;

export function CountUp({ value, formatter, className }: CountUpProps) {
  const nodeRef = useRef<HTMLSpanElement>(null);
  const prevValueString = useRef(value.toString());
  // Track the current animated value to support smooth interruptions
  const currentAnimatedValue = useRef(Number(value));
  const lastChangeAtMs = useRef<number | undefined>(undefined);
  const averageGapSecs = useRef<number | undefined>(undefined);
  // Callers commonly pass an inline formatter, so its identity changes on every render of the
  // parent. Reading it from a ref keeps the running tween out of the effect's dependencies while
  // still formatting with the latest function.
  const formatterRef = useRef(formatter);
  useEffect(() => {
    formatterRef.current = formatter;
  }, [formatter]);

  useEffect(() => {
    const node = nodeRef.current;
    if (!node) return;

    // If value hasn't changed effectively, do nothing (handling bigint/number comparison via string)
    if (value.toString() === prevValueString.current) return;

    // Start from the current animated value to avoid jumps
    const start = currentAnimatedValue.current;
    const target = Number(value);

    // Update tracking ref for next comparison
    prevValueString.current = value.toString();

    const now = performance.now();
    const previous = lastChangeAtMs.current;
    lastChangeAtMs.current = now;
    let duration = FIRST_TWEEN_SECS;
    if (previous !== undefined) {
      const gap = Math.min(
        MAX_GAP_SECS,
        Math.max(MIN_GAP_SECS, (now - previous) / 1000),
      );
      const average =
        averageGapSecs.current === undefined
          ? gap
          : averageGapSecs.current +
            GAP_SMOOTHING * (gap - averageGapSecs.current);
      averageGapSecs.current = average;
      duration = average * TWEEN_STRETCH;
    }

    const controls = animate(start, target, {
      duration,
      ease: 'linear',
      onUpdate: (latest) => {
        // Track the current value so we can resume from here if interrupted
        currentAnimatedValue.current = latest;

        if (nodeRef.current) {
          const format = formatterRef.current;
          nodeRef.current.textContent = format
            ? format(latest)
            : Math.round(latest).toString();
        }
      },
    });

    return () => controls.stop();
  }, [value]);

  // Initial render text
  const initialText = formatter ? formatter(value) : value.toString();

  return (
    <span ref={nodeRef} className={className}>
      {initialText}
    </span>
  );
}
