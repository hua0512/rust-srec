import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { PlatformSpecificTab } from '../../platform-specific-tab';

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
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return {
    stored: () => form.getValues('platform_specific_config'),
  };
}

const cdnInput = () =>
  screen.getByLabelText('Preferred CDN') as HTMLInputElement;
const retriesInput = () =>
  screen.getByLabelText('API Request Retries') as HTMLInputElement;

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

describe('Douyu configuration', () => {
  it('leaves the fields empty when nothing is set and names the fallback', () => {
    renderFields();

    expect(cdnInput().value).toBe('');
    expect(cdnInput().placeholder).toBe('Default: hw');
    expect(retriesInput().value).toBe('');
    expect(retriesInput().placeholder).toBe('Default: 3');
  });

  it('offers inheritance instead of a default on an overriding layer', () => {
    renderFields(undefined, { inherited: true });

    expect(cdnInput().placeholder).toBe('Inherited');
    expect(retriesInput().placeholder).toBe('Inherited');
  });

  it('shows the saved values', () => {
    renderFields({ cdn: 'hw-h5', request_retries: 5 });

    expect(cdnInput().value).toBe('hw-h5');
    expect(retriesInput().value).toBe('5');
  });

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

  it('shows the legacy CDN default for web extraction', () => {
    renderFields({ api_mode: 'web' });
    expect(cdnInput().placeholder).toBe('Default: ws-h5');
    expect(screen.getByLabelText('Extraction Method')).toHaveTextContent(
      'Web (deprecated)',
    );
  });

  it('hides the App device options for web extraction', () => {
    renderFields({ api_mode: 'web' });
    expect(screen.queryByText('App Device')).toBeNull();
    expect(screen.queryByLabelText('Device Model')).toBeNull();
  });

  it('shows the App device options when the method is unset', () => {
    renderFields();
    expect(screen.getByText('App Device')).toBeInTheDocument();
  });

  it('shows the legacy CDN default for audio-only extraction', () => {
    renderFields({ api_mode: 'app', only_audio: true });
    expect(cdnInput().placeholder).toBe('Default: ws-h5');
  });

  it('preserves unset method and codec instead of writing overrides', () => {
    const { stored } = renderFields({}, { inherited: true });
    expect(screen.getByLabelText('Extraction Method')).toHaveTextContent(
      'Inherited',
    );
    expect(screen.getByLabelText('Preferred Video Codec')).toHaveTextContent(
      'Inherited',
    );
    expect(stored().api_mode).toBeUndefined();
    expect(stored().codec).toBeUndefined();
  });

  it('changes method and codec and restores inheritance', () => {
    const { stored } = renderFields(
      { api_mode: 'app', codec: 'avc' },
      { inherited: true },
    );
    const method = screen.getByLabelText('Extraction Method');
    expect(method).toHaveTextContent('Android App');
    fireEvent.keyDown(method, { key: 'ArrowDown' });
    fireEvent.click(screen.getByRole('option', { name: 'Web (deprecated)' }));
    expect(stored().api_mode).toBe('web');

    const codec = screen.getByLabelText('Preferred Video Codec');
    expect(codec).toHaveTextContent('AVC (H.264)');
    fireEvent.keyDown(codec, { key: 'ArrowDown' });
    fireEvent.click(screen.getByRole('option', { name: 'HEVC (H.265)' }));
    expect(stored().codec).toBe('hevc');

    for (const trigger of [method, codec]) {
      fireEvent.keyDown(trigger, { key: 'ArrowDown' });
      fireEvent.click(screen.getByRole('option', { name: 'Inherited' }));
    }
    expect(stored().api_mode).toBeNull();
    expect(stored().codec).toBeNull();
  });

  it('saves device options and can clear them to inherit', () => {
    const { stored } = renderFields({}, { inherited: true });
    for (const [label, key, value] of [
      ['Device Model', 'device_name', 'OnePlus 12'],
      ['Android Version', 'os_version', '15'],
      ['Device ID', 'device_id', '0123456789abcdef0123456789abcdef'],
    ]) {
      const input = screen.getByLabelText(label);
      expect(input).toHaveAttribute('placeholder', 'Inherited');
      fireEvent.change(input, { target: { value } });
      expect(stored()[key]).toBe(value);
      fireEvent.change(input, { target: { value: '' } });
      expect(stored()[key]).toBeNull();
    }
    const source = screen.getByLabelText('Device ID source');
    fireEvent.keyDown(source, { key: 'ArrowDown' });
    fireEvent.click(
      screen.getByRole('option', { name: 'Server registration' }),
    );
    expect(stored().device_id_mode).toBe('server');
    fireEvent.keyDown(source, { key: 'ArrowDown' });
    fireEvent.click(screen.getByRole('option', { name: 'Inherited' }));
    expect(stored().device_id_mode).toBeNull();
  });
});
