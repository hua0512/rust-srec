import { fireEvent, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { PlatformSpecificTab } from '../../platform-specific-tab';
import { renderWithProviders, chooseOption } from '@/test/render';

type Values = { platform_specific_config: Record<string, unknown> };

function renderFields(
  options?: Record<string, unknown>,
  { inherited = false } = {},
) {
  let form!: UseFormReturn<Values>;
  function Harness() {
    form = useForm<Values>({
      defaultValues: { platform_specific_config: options },
    });
    return (
      <Form {...form}>
        <PlatformSpecificTab
          form={form}
          platformName="douyu"
          inherited={inherited}
        />
      </Form>
    );
  }
  renderWithProviders(<Harness />);
  return {
    stored: () => form.getValues('platform_specific_config'),
  };
}

const cdnInput = () =>
  screen.getByLabelText('Preferred CDN') as HTMLInputElement;
const retriesInput = () =>
  screen.getByLabelText('API Request Retries') as HTMLInputElement;

describe('Douyu configuration', () => {
  it('can be emptied again', () => {
    const { stored } = renderFields({ cdn: 'hw-h5', request_retries: 5 });

    fireEvent.change(cdnInput(), { target: { value: '' } });
    fireEvent.change(retriesInput(), { target: { value: '' } });

    expect(cdnInput().value).toBe('');
    expect(retriesInput().value).toBe('');
    // Null rather than an absent key, so the cleared value still overrides a
    // legacy flat template setting instead of falling through to it.
    expect(stored().cdn).toBeNull();
    expect(stored().request_retries).toBeNull();
  });

  it('keeps a retry count of zero', () => {
    const { stored } = renderFields({ request_retries: 5 });

    fireEvent.change(retriesInput(), { target: { value: '0' } });

    expect(stored().request_retries).toBe(0);
  });

  it('changes method and codec and restores inheritance', async () => {
    const { stored } = renderFields(
      { api_mode: 'app', codec: 'avc' },
      { inherited: true },
    );
    const method = screen.getByLabelText('Extraction Method');
    expect(method).toHaveTextContent('Android App');
    await chooseOption(method, 'Web (deprecated)');
    expect(stored().api_mode).toBe('web');

    const codec = screen.getByLabelText('Preferred Video Codec');
    expect(codec).toHaveTextContent('AVC (H.264)');
    await chooseOption(codec, 'HEVC (H.265)');
    expect(stored().codec).toBe('hevc');

    for (const trigger of [method, codec]) {
      await chooseOption(trigger, 'Inherited');
    }
    expect(stored().api_mode).toBeNull();
    expect(stored().codec).toBeNull();
  });
});
