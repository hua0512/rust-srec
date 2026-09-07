import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { NotificationChannel } from '@/api/schemas';
import {
  getSubscriptions,
  listEventTypes,
  updateSubscriptions,
} from '@/server/functions/notifications';
import { SubscriptionManager } from '../subscription-manager';

vi.mock('@/server/functions/notifications', () => ({
  listEventTypes: vi.fn(),
  getSubscriptions: vi.fn(),
  updateSubscriptions: vi.fn(),
}));

const channel: NotificationChannel = {
  id: '11111111-1111-1111-1111-111111111111',
  name: 'Ops',
  channel_type: 'Webhook',
  settings: '{}',
};

const eventTypes = [
  { event_type: 'stream_online', label: 'Stream online', priority: 5 },
  { event_type: 'download_error', label: 'Download error', priority: 8 },
];

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(listEventTypes).mockResolvedValue(eventTypes as any);
  vi.mocked(getSubscriptions).mockResolvedValue(['stream_online'] as any);
  vi.mocked(updateSubscriptions).mockResolvedValue(undefined as any);
});

async function renderManager() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <QueryClientProvider client={queryClient}>
        <SubscriptionManager channel={channel} open onOpenChange={() => {}} />
      </QueryClientProvider>
    </I18nProvider>,
  );
  await screen.findByText('download_error');
  return { queryClient };
}

const checkboxes = () => screen.getAllByRole('checkbox');
const saveButton = () => screen.getByRole('button', { name: /Save/ });

describe('SubscriptionManager', () => {
  it('starts from the saved subscriptions', async () => {
    await renderManager();

    expect(checkboxes()[0]).toBeChecked();
    expect(checkboxes()[1]).not.toBeChecked();
  });

  // The subscriptions query refetches whenever the window regains focus, and
  // adopting its answer a second time would silently undo pending choices.
  it('keeps unsaved choices when the saved subscriptions arrive again', async () => {
    const { queryClient } = await renderManager();

    fireEvent.click(checkboxes()[1]);
    expect(checkboxes()[1]).toBeChecked();

    // What returning to the window does.
    await queryClient.refetchQueries();

    await waitFor(() => expect(getSubscriptions).toHaveBeenCalledTimes(2));
    expect(checkboxes()[0]).toBeChecked();
    expect(checkboxes()[1]).toBeChecked();
  });

  it('saves the selection that is on screen', async () => {
    await renderManager();

    fireEvent.click(checkboxes()[0]);
    fireEvent.click(checkboxes()[1]);
    fireEvent.click(saveButton());

    await waitFor(() => expect(updateSubscriptions).toHaveBeenCalled());
    expect(vi.mocked(updateSubscriptions).mock.calls[0][0]).toEqual({
      data: { id: channel.id, events: ['download_error'] },
    });
  });
});
