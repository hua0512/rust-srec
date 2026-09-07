import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useForm, type UseFormReturn } from 'react-hook-form';

import { Form } from '@/components/ui/form';
import { MetadataConfigForm } from '../metadata-config-form';

function renderForm(custom?: Record<string, string>) {
  let form!: UseFormReturn<any>;
  function Harness() {
    form = useForm<any>({ defaultValues: { custom } });
    return (
      <Form {...form}>
        <MetadataConfigForm control={form.control} />
      </Form>
    );
  }
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <Harness />
    </I18nProvider>,
  );
  return {
    stored: () => form.getValues('custom') as Record<string, string>,
    form: () => form,
  };
}

const keyInputs = () =>
  screen.queryAllByPlaceholderText('Key') as HTMLInputElement[];
const valueInputs = () =>
  screen.queryAllByPlaceholderText('Value') as HTMLInputElement[];
const addTag = () =>
  fireEvent.click(screen.getByRole('button', { name: /Add Custom Tag/ }));
/** The remove control is the only button inside a tag row. */
const removeRow = (index: number) =>
  fireEvent.click(keyInputs()[index].parentElement!.querySelector('button')!);

describe('metadata custom tags', () => {
  it('lists the saved tags', () => {
    renderForm({ genre: 'live', season: '2' });

    expect(keyInputs().map((input) => input.value)).toEqual([
      'genre',
      'season',
    ]);
    expect(valueInputs().map((input) => input.value)).toEqual(['live', '2']);
  });

  it('keeps two fresh rows apart instead of collapsing them', () => {
    renderForm();

    addTag();
    addTag();

    expect(keyInputs()).toHaveLength(2);
  });

  it('accepts a name typed one character at a time', () => {
    const { stored } = renderForm();
    addTag();

    fireEvent.change(keyInputs()[0], { target: { value: 'g' } });
    fireEvent.change(keyInputs()[0], { target: { value: 'ge' } });
    fireEvent.change(keyInputs()[0], { target: { value: 'gen' } });

    expect(keyInputs()[0].value).toBe('gen');
    expect(stored()).toEqual({ gen: '' });
  });

  it('saves only the rows that have a name', () => {
    const { stored } = renderForm();
    addTag();
    addTag();

    fireEvent.change(keyInputs()[0], { target: { value: 'genre' } });
    fireEvent.change(valueInputs()[0], { target: { value: 'live' } });
    fireEvent.change(valueInputs()[1], { target: { value: 'orphan' } });

    expect(keyInputs()).toHaveLength(2);
    expect(stored()).toEqual({ genre: 'live' });
  });

  // A row removed by index would leave the survivor rendered on its neighbour's
  // element, which is what moves the caret out of the row being typed in.
  it('removes the row that was asked for and leaves the other one in place', () => {
    const { stored } = renderForm({ genre: 'live', season: '2' });
    const second = keyInputs()[1];

    removeRow(0);

    expect(keyInputs()).toEqual([second]);
    expect(stored()).toEqual({ season: '2' });
  });

  it('adopts tags loaded from outside the editor', () => {
    const { form } = renderForm({ genre: 'live' });
    addTag();

    act(() => form().reset({ custom: { season: '2' } }));

    expect(keyInputs().map((input) => input.value)).toEqual(['season']);
  });
});
