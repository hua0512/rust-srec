import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import type { PropsWithChildren } from 'react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import type { JobPreset } from '@/api/schemas';
import { PresetMetaForm } from '../editor/preset-meta-form';
import type { PresetFormValues } from '../preset-editor';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: PropsWithChildren) => <a href="#">{children}</a>,
}));

vi.mock('motion/react', () => ({
  motion: {
    div: ({ children }: PropsWithChildren) => <div>{children}</div>,
  },
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

const EXISTING_PRESET: JobPreset = {
  id: 'preset-default-metadata',
  name: 'add_metadata',
  description: 'Add metadata',
  category: 'metadata',
  processor: 'metadata',
  config: { title: 'Example' },
  created_at: '2024-01-01T00:00:00Z',
  updated_at: '2024-01-01T00:00:00Z',
};

function PresetMetaFormHarness() {
  const form = useForm<PresetFormValues>({
    defaultValues: {
      id: '',
      name: '',
      description: '',
      category: '',
      processor: 'remux',
      config: { mode: 'copy' },
    },
  });
  const processor = form.watch('processor');

  useEffect(() => {
    form.reset({
      id: 'preset-default-metadata',
      name: 'add_metadata',
      description: 'Add metadata',
      category: 'metadata',
      processor: 'metadata',
      config: { title: 'Example' },
    });
  }, [form]);

  return (
    <I18nProvider i18n={i18n}>
      <Form {...form}>
        <form>
          <PresetMetaForm
            form={form}
            initialData={EXISTING_PRESET}
            title="Edit preset"
            isUpdating={false}
          />
          <output data-testid="processor">{processor}</output>
          <output data-testid="config">
            {JSON.stringify(form.watch('config'))}
          </output>
        </form>
      </Form>
    </I18nProvider>
  );
}

describe('preset processor select', () => {
  it('preserves reset processor and config values', async () => {
    render(<PresetMetaFormHarness />);

    await waitFor(() => {
      expect(screen.getByTestId('processor')).toHaveTextContent('metadata');
    });
    expect(screen.getByTestId('config')).toHaveTextContent(
      JSON.stringify({ title: 'Example' }),
    );
  });
});
