import { screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import type { PropsWithChildren } from 'react';
import { useForm } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import type { JobPreset } from '@/api/schemas';
import { PresetMetaForm } from '../editor/preset-meta-form';
import type { PresetFormValues } from '../preset-editor';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

vi.mock('motion/react', () => ({
  motion: {
    div: ({ children }: PropsWithChildren) => <div>{children}</div>,
  },
}));

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
  );
}

describe('preset processor select', () => {
  it('preserves reset processor and config values', async () => {
    renderWithProviders(<PresetMetaFormHarness />);

    await waitFor(() => {
      expect(screen.getByTestId('processor')).toHaveTextContent('metadata');
    });
    expect(screen.getByTestId('config')).toHaveTextContent(
      JSON.stringify({ title: 'Example' }),
    );
  });
});
