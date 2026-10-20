import { useEffect, useState } from 'react';

/**
 * The current time in epoch milliseconds, re-read every second while the
 * calling component is mounted. Each caller re-renders on every tick, so keep
 * it in the smallest component that shows a running time.
 */
export function useNow(): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return now;
}
