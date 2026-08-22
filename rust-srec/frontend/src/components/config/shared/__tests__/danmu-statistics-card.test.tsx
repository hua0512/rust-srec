import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import { useForm } from 'react-hook-form';

import { DanmuStatisticsCard } from '../danmu-statistics-card';
import { Form } from '@/components/ui/form';

type StopWords = string[] | undefined;

function Harness({
  expose,
  initial,
}: {
  expose: (read: () => StopWords) => void;
  initial?: StopWords;
}) {
  const form = useForm({
    defaultValues: {
      danmu_statistics: initial ? { extra_stop_words: initial } : {},
    },
  });
  expose(
    () => form.getValues('danmu_statistics.extra_stop_words') as StopWords,
  );
  return (
    <Form {...form}>
      <DanmuStatisticsCard form={form} />
    </Form>
  );
}

function renderCard(initial?: StopWords) {
  const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
  let read: () => StopWords = () => undefined;

  render(
    <I18nProvider i18n={i18n}>
      <Harness expose={(fn) => (read = fn)} initial={initial} />
    </I18nProvider>,
  );

  const textarea = screen.getByPlaceholderText(
    'One word per line',
  ) as HTMLTextAreaElement;
  return { textarea, stopWords: () => read() };
}

/** Simulate the DOM value the user would have after typing. */
function type(textarea: HTMLTextAreaElement, value: string) {
  fireEvent.change(textarea, { target: { value } });
}

describe('DanmuStatisticsCard ignored words', () => {
  // The field is controlled from the form value, so normalizing on every
  // keystroke rewrites what the user is typing. Trimming and dropping blanks
  // mid-edit erases the newline that starts the next entry, which makes a
  // second word impossible to type at all.
  it('keeps a trailing newline so a second word can be started', () => {
    const { textarea } = renderCard(['lol']);

    type(textarea, 'lol\n');

    expect(textarea.value).toBe('lol\n');
  });

  it('accepts a second word on the new line', () => {
    const { textarea, stopWords } = renderCard(['lol']);

    type(textarea, 'lol\n');
    type(textarea, 'lol\nnice');

    expect(textarea.value).toBe('lol\nnice');
    expect(stopWords()).toEqual(['lol', 'nice']);
  });

  // Leading whitespace is a normal intermediate state while typing; the
  // backend's `sanitized()` trims on save, so the field must not fight it.
  it('leaves whitespace alone while typing', () => {
    const { textarea } = renderCard();

    type(textarea, 'a\n  b');

    expect(textarea.value).toBe('a\n  b');
  });

  it('stores trimmed words without the blank lines', () => {
    const { textarea, stopWords } = renderCard();

    type(textarea, 'one\n\n  two  \n');

    expect(stopWords()).toEqual(['one', 'two']);
  });

  it('clears the value when emptied so the layer inherits again', () => {
    const { textarea, stopWords } = renderCard(['lol']);

    type(textarea, '');

    expect(stopWords()).toBeUndefined();
  });

  it('shows words loaded from an existing config', () => {
    const { textarea } = renderCard(['lol', 'nice']);

    expect(textarea.value).toBe('lol\nnice');
  });
});
