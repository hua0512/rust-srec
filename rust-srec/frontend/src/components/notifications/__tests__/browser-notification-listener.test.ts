import { describe, expect, it } from 'vitest';

import { formatBrowserNotification } from '../browser-notification-listener';
import type { NotificationEventLog } from '@/api/schemas/notifications';
import { testI18n } from '@/test/render';

function log(payload: string): NotificationEventLog {
  return {
    id: '00000000-0000-4000-8000-000000000000',
    event_type: 'credential_invalid',
    priority: 10,
    payload,
    created_at: 0,
  };
}

describe('formatBrowserNotification', () => {
  it('shows the reason of a credential event, whose fields sit under `event`', () => {
    const payload = JSON.stringify({
      Credential: {
        event: {
          type: 'invalid',
          platform: 'bilibili',
          reason: 'Cookie expired',
        },
      },
    });

    expect(formatBrowserNotification(log(payload), testI18n()).body).toBe(
      'Cookie expired',
    );
  });
});
