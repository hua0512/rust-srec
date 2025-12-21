import { useEffect, useRef } from 'react';
import { animate } from 'motion';

interface CountUpProps {
  value: number | bigint;
  formatter?: (value: number | bigint) => string;
  className?: string;
  duration?: number;
}

export function CountUp({
  value,
  formatter,
  className,
  duration = 0.8,
}: CountUpProps) {
  const nodeRef = useRef<HTMLSpanElement>(null);
  const prevValueString = useRef(value.toString());
  // Track the current animated value to support smooth interruptions
  const currentAnimatedValue = useRef(Number(value));

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

    const controls = animate(start, target, {
      duration,
      ease: 'easeOut',
      onUpdate: (latest) => {
        // Track the current value so we can resume from here if interrupted
        currentAnimatedValue.current = latest;

        if (nodeRef.current) {
          nodeRef.current.textContent = formatter
            ? formatter(latest)
            : Math.round(latest).toString();
        }
      },
    });

    return () => controls.stop();
  }, [value, duration, formatter]);

  // Initial render text
  const initialText = formatter ? formatter(value) : value.toString();

  return (
    <span ref={nodeRef} className={className}>
      {initialText}
    </span>
  );
}
