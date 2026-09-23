import { useCallback, useRef, useState } from 'react';

export function useInView(options?: IntersectionObserverInit) {
  const observerRef = useRef<IntersectionObserver | null>(null);
  const [inView, setInView] = useState(false);

  const ref = useCallback(
    (node: Element | null) => {
      observerRef.current?.disconnect();
      if (!node) return;

      observerRef.current = new IntersectionObserver(([entry]) => {
        setInView(Boolean(entry?.isIntersecting));
      }, options);

      observerRef.current.observe(node);
    },
    [options],
  );

  return { ref, inView };
}
