import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import type { PropsWithChildren } from 'react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { PresetMetaForm } from '../editor/preset-meta-form';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: PropsWithChildren) => <a href="#">{children}</a>,
}));

vi.mock('motion/react', () => ({
  motion: {
    div: ({ children }: PropsWithChildren) => <div>{children}</div>,
  },
}));

interface PresetValues {
  id: string;
  name: string;
  description: string;
  category: string;
  processor: string;
  config: Record<string, unknown>;
}

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

function PresetMetaFormHarness() {
  const form = useForm<PresetValues>({
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
            initialData={{ id: 'preset-default-metadata' }}
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
