import { fireEvent, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import type { TemplateFormValues } from '../../template-editor';
import { PlatformOverrideCard } from '../platform-override-card';
import { renderWithProviders } from '@/test/render';

function renderCard() {
  function Harness() {
    const form = useForm<TemplateFormValues>({
      defaultValues: { name: 'Test', platform_overrides: { huya: {} } },
    });
    return (
      <Form {...form}>
        <PlatformOverrideCard
          form={form}
          platformName="huya"
          onRemove={() => {}}
        />
      </Form>
    );
  }
  renderWithProviders(<Harness />);
}

describe('PlatformOverrideCard', () => {
  // The resolver reads only the extractor options and the pipelines from a template's
  // per-platform override, so offering any other setting there would save it for nothing.
  it('offers only the settings a per-platform override applies', () => {
    renderCard();

    fireEvent.click(screen.getByText('Huya'));

    expect(
      screen.getAllByRole('tab').map((tab) => tab.textContent?.trim()),
    ).toEqual(['Accounts', 'Specific', 'Pipeline']);
  });
});
