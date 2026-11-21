import { renderToStaticMarkup } from 'react-dom/server';
import { render, screen } from '@testing-library/react';

import { ColorPicker } from '../color-picker';

/** Serve `values` for the document element only; anything else (jsdom's own
 *  style queries, testing-library) keeps the real implementation. */
function stubComputedStyle(values: Record<string, string>) {
  const real = window.getComputedStyle.bind(window);
  vi.spyOn(window, 'getComputedStyle').mockImplementation(
    (element: Element, pseudo?: string | null) => {
      if (element !== document.documentElement) return real(element, pseudo);
      return {
        getPropertyValue: (property: string) => values[property] ?? '',
      } as CSSStyleDeclaration;
    },
  );
}

function renderPicker(props: { value?: string; themeKey?: string } = {}) {
  return render(
    <ColorPicker
      label="Primary"
      cssVar="--primary"
      value={props.value ?? ''}
      themeKey={props.themeKey ?? 'preset:default:none:light'}
      onChange={vi.fn()}
    />,
  );
}

function textInput() {
  return screen.getByPlaceholderText('--primary value') as HTMLInputElement;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('color picker', () => {
  it('renders no colour before effects run, so hydration matches the server', () => {
    stubComputedStyle({ '--primary': '#123456' });

    const markup = renderToStaticMarkup(
      <ColorPicker
        label="Primary"
        cssVar="--primary"
        value=""
        themeKey="preset:default:none:light"
        onChange={vi.fn()}
      />,
    );

    expect(markup).toContain('background-color:transparent');
    expect(markup).toContain('value=""');
    expect(markup).not.toContain('#123456');
  });

  it('re-reads the variable when the active theme changes', () => {
    const values: Record<string, string> = { '--primary': '#111111' };
    stubComputedStyle(values);

    const { rerender } = renderPicker();
    expect(textInput().value).toBe('#111111');

    values['--primary'] = '#222222';
    rerender(
      <ColorPicker
        label="Primary"
        cssVar="--primary"
        value=""
        themeKey="preset:amber:none:dark"
        onChange={vi.fn()}
      />,
    );

    expect(textInput().value).toBe('#222222');
  });
});
