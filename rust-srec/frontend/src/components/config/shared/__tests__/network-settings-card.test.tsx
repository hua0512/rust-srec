import { screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { NetworkSettingsCard } from '../network-settings-card';
import { renderWithProviders } from '@/test/render';

function Harness() {
  const form = useForm<{ download_retry_policy: string | null }>({
    defaultValues: { download_retry_policy: null },
  });
  return (
    <Form {...form}>
      <NetworkSettingsCard
        form={form}
        paths={{ retryPolicy: 'download_retry_policy' }}
        configMode="json"
      />
    </Form>
  );
}

describe('NetworkSettingsCard', () => {
  it('points a template to its platform overrides for account choice', () => {
    renderWithProviders(<Harness />);
    expect(
      screen.getByText(/Accounts are chosen per platform/),
    ).toBeInTheDocument();
  });
});
