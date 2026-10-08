import { useEffect, useState } from 'react';

/**
 * Keeps an element mounted for its exit animation after `present` turns
 * false; `exitMs` must match that animation's duration. Mounting needs no
 * delay: the enter animation runs on mount.
 */
export function usePresence(present: boolean, exitMs: number) {
  const [lingering, setLingering] = useState(present);
  if (present && !lingering) setLingering(true);
  useEffect(() => {
    if (present) return;
    const timer = setTimeout(() => setLingering(false), exitMs);
    return () => clearTimeout(timer);
  }, [present, exitMs]);
  return { mounted: present || lingering, exiting: !present && lingering };
}
