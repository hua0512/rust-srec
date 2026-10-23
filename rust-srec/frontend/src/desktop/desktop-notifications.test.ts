import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { afterEach, expect, it, vi } from 'vitest';

import { testDesktopNotifications } from './desktop-notifications';

afterEach(clearMocks);

it('waits for native test delivery and propagates its failure instead of just emitting an event', async () => {
  let rejectDelivery!: (reason: string) => void;
  const delivery = new Promise<void>((_resolve, reject) => {
    rejectDelivery = reject;
  });
  const invoked = vi.fn((command: string) => {
    if (command === 'test_desktop_notifications') return delivery;
    // Emitting the old fire-and-forget event succeeds even when delivery fails.
    return undefined;
  });
  mockIPC(invoked);

  const settled = vi.fn();
  const result = testDesktopNotifications();
  const failure = expect(result).rejects.toBe(
    'Windows could not submit the notification',
  );
  void result.then(settled, settled);
  await vi.waitFor(() => expect(invoked).toHaveBeenCalled());
  expect(settled).not.toHaveBeenCalled();

  rejectDelivery('Windows could not submit the notification');
  await failure;
});
