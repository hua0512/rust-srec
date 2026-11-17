import { screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';
import { describe, expect, it, vi } from 'vitest';

import { platformAccountsQueryOptions } from '@/api/credential-profiles';
import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import type { BigoNotificationStatus } from '@/api/schemas/platform-configs';
import { Form } from '@/components/ui/form';
import { newQueryClient, renderWithProviders } from '@/test/render';
import { PlatformSpecificTab } from '../../platform-specific-tab';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

type Values = { platform_specific_config: Record<string, unknown> };

const status: BigoNotificationStatus = {
  platform_id: 'platform-bigo',
  state: 'disabled',
  reason: 'notifications_off',
  message: null,
  subscribed: 0,
  total: 0,
  streamers: [],
};

function renderFields(
  options: Record<string, unknown>,
  { inherited = false } = {},
) {
  const client = newQueryClient();
  client.setQueryData(bigoNotificationStatusQueryOptions.queryKey, status);
  client.setQueryData(
    platformAccountsQueryOptions('platform-bigo').queryKey,
    [],
  );
  function Harness() {
    const form = useForm<Values>({
      defaultValues: { platform_specific_config: options },
    });
    return (
      <Form {...form}>
        <PlatformSpecificTab
          form={form}
          platformName="bigo"
          inherited={inherited}
        />
      </Form>
    );
  }
  renderWithProviders(<Harness />, { client });
}

const notice = () => screen.queryByText(/Live notifications need a signed-in/);
const warning = () => screen.queryByText(/checked by polling, which is/);
const picker = () => screen.queryByText('Notification Account');

describe('Bigo live notification settings', () => {
  it('warns about polling while notifications are off', () => {
    renderFields({});

    expect(warning()).toBeInTheDocument();
    expect(picker()).not.toBeInTheDocument();
    expect(notice()).not.toBeInTheDocument();
  });

  it('explains the account before one is chosen', () => {
    renderFields({ batch_detection: true });

    expect(picker()).toBeInTheDocument();
    expect(notice()).toBeInTheDocument();
    expect(warning()).toBeInTheDocument();
  });

  it('drops both notes once an account is chosen', () => {
    renderFields({ batch_detection: true, notification_account: 'profile-a' });

    expect(picker()).toBeInTheDocument();
    expect(notice()).not.toBeInTheDocument();
    expect(warning()).not.toBeInTheDocument();
  });

  it('offers only the switch on a template or streamer', () => {
    renderFields({ batch_detection: true }, { inherited: true });

    expect(screen.getByText('Batch Detection')).toBeInTheDocument();
    expect(picker()).not.toBeInTheDocument();
    expect(notice()).not.toBeInTheDocument();
    expect(warning()).not.toBeInTheDocument();
  });
});
