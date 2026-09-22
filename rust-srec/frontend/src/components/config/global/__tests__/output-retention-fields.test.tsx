import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';
import { Form } from '@/components/ui/form';
import { OutputRetentionFields } from '../output-retention-fields';

beforeAll(() => {
  // Radix measures tooltip content; jsdom does not provide ResizeObserver.
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});

afterAll(() => vi.unstubAllGlobals());

function renderFields(days = 0, deleteFiles = false) {
  let values = () => ({
    output_retention_days: days,
    output_retention_delete_files: deleteFiles,
  });
  function Harness() {
    const form = useForm({
      defaultValues: {
        output_retention_days: days,
        output_retention_delete_files: deleteFiles,
      },
    });
    values = () => form.getValues();
    return (
      <Form {...form}>
        <OutputRetentionFields />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return { values: () => values() };
}

async function openInfo(label: string) {
  const trigger = screen
    .getByText(label)
    .querySelector('[data-slot="tooltip-trigger"]');
  expect(trigger).not.toBeNull();
  fireEvent.pointerMove(trigger!, { pointerType: 'mouse' });
  return screen.findByRole('tooltip');
}

describe('output retention settings', () => {
  it('starts disabled and explains records-only cleanup in a tooltip', async () => {
    renderFields();
    expect(
      screen.getByRole('spinbutton', { name: 'Output retention (days)' }),
    ).toHaveValue(0);
    expect(
      screen.getByRole('combobox', { name: 'When outputs expire' }),
    ).toBeDisabled();
    expect(
      screen.queryByText(/automatic cleanup can no longer delete/),
    ).not.toBeInTheDocument();
    expect(await openInfo('Output retention (days)')).toHaveTextContent(
      /Set to 0 to disable automatic output cleanup/,
    );
    fireEvent.keyDown(document.body, { key: 'Escape' });
    expect(await openInfo('When outputs expire')).toHaveTextContent(
      /automatic cleanup can no longer delete/,
    );
  });

  it('lets the user enable retention and choose either deletion mode', async () => {
    const { values } = renderFields();
    fireEvent.change(screen.getByRole('spinbutton'), {
      target: { value: '14' },
    });
    const select = screen.getByRole('combobox');
    expect(select).toBeEnabled();
    fireEvent.keyDown(select, { key: 'ArrowDown' });
    fireEvent.click(
      screen.getByRole('option', { name: 'Delete records and files' }),
    );
    expect(values()).toEqual({
      output_retention_days: 14,
      output_retention_delete_files: true,
    });
    expect(
      screen.queryByText(/Permanently deletes tracked output files/),
    ).not.toBeInTheDocument();
    expect(await openInfo('When outputs expire')).toHaveTextContent(
      /Permanently deletes tracked output files/,
    );
    fireEvent.keyDown(document.body, { key: 'Escape' });
    fireEvent.keyDown(select, { key: 'ArrowDown' });
    fireEvent.click(
      screen.getByRole('option', { name: 'Delete records only' }),
    );
    expect(values().output_retention_delete_files).toBe(false);
  });

  it('loads a saved file-deletion choice and retains it while cleanup is disabled', () => {
    const { values } = renderFields(7, true);
    expect(screen.getByRole('combobox')).toHaveTextContent(
      'Delete records and files',
    );
    fireEvent.change(screen.getByRole('spinbutton'), {
      target: { value: '0' },
    });
    expect(screen.getByRole('combobox')).toBeDisabled();
    expect(values()).toEqual({
      output_retention_days: 0,
      output_retention_delete_files: true,
    });
  });
});
