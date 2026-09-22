import { cleanup, renderHook } from '@testing-library/react';
import type { PropsWithChildren } from 'react';

import { WebSocketContext } from '@/providers/WebSocketContext';
import { useDownloadProgress } from '../use-download-progress';

afterEach(cleanup);

function setup(streamerId?: string, strict = false) {
  const active = new Set<string>();
  const events: string[] = [];
  const context = {
    isConnected: true,
    subscribe(id: string) {
      events.push(`subscribe:${id}`);
      active.add(id);
    },
    unsubscribe(id: string) {
      events.push(`unsubscribe:${id}`);
      active.delete(id);
    },
  };
  function Wrapper({ children }: PropsWithChildren) {
    return (
      <WebSocketContext.Provider value={context}>
        {children}
      </WebSocketContext.Provider>
    );
  }
  const hook = renderHook(
    ({ id }: { id: string | undefined }) =>
      useDownloadProgress({ streamerId: id }),
    {
      initialProps: { id: streamerId },
      wrapper: Wrapper,
      reactStrictMode: strict,
    },
  );
  return { ...hook, active, events };
}

it('replaces the subscription once on navigation and releases it on unmount', () => {
  const { rerender, unmount, active, events } = setup('first');
  rerender({ id: 'second' });
  expect(active).toEqual(new Set(['second']));
  expect(events).toEqual([
    'subscribe:first',
    'unsubscribe:first',
    'subscribe:second',
  ]);
  unmount();
  expect(active.size).toBe(0);
  expect(events.at(-1)).toBe('unsubscribe:second');
});

it('leaves no subscription when the streamer is absent', () => {
  const { rerender, active, events } = setup();
  expect(events).toEqual([]);
  rerender({ id: 'first' });
  rerender({ id: undefined });
  expect(active.size).toBe(0);
  expect(events).toEqual(['subscribe:first', 'unsubscribe:first']);
});

it('keeps the subscription active after Strict Mode replays the effect', () => {
  const { active, unmount } = setup('first', true);
  expect(active).toEqual(new Set(['first']));
  unmount();
  expect(active.size).toBe(0);
});
