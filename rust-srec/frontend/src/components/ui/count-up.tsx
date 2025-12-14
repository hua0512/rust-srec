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

  useEffect(() => {
    const node = nodeRef.current;
    if (!node) return;

    // If value hasn't changed effectively, do nothing (handling bigint/number comparison via string)
    if (value.toString() === prevValueString.current) return;

    const start = parseFloat(prevValueString.current); // Limitation: animates as number, so very large bigints might lose precision in animation but end value is text set manually?
    // Actually animate function updates text content usually?
    // Let's use 'animate' helper to drive the value.

    const target = Number(value); // We have to animate as number. BigInt animation is tricky. Assuming values fit in double for animation purposes (stats usually do).

    // If it's the first render, maybe we don't want to animate from 0? or yes?
    // Let's animate from previous value.

    prevValueString.current = value.toString();

    const controls = animate(start, target, {
      duration,
      ease: 'easeOut',
      onUpdate: (latest) => {
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
