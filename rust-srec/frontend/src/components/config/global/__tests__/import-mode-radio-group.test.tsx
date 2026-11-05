import { fireEvent, screen } from '@testing-library/react';
import { useState } from 'react';

import {
  ImportModeRadioGroup,
  type ImportMode,
} from '../import-mode-radio-group';
import { renderWithProviders } from '@/test/render';

function renderGroup() {
  function Harness() {
    const [mode, setMode] = useState<ImportMode>('merge');
    return <ImportModeRadioGroup value={mode} onValueChange={setMode} />;
  }

  renderWithProviders(<Harness />);
}

const merge = () => screen.getByRole('radio', { name: /Merge Changes/ });
const replace = () => screen.getByRole('radio', { name: /Replace All/ });

describe('import mode radio group', () => {
  it('announces the strategies as one labelled choice', () => {
    renderGroup();

    const group = screen.getByRole('radiogroup', { name: 'Import Strategy' });

    expect(group).toContainElement(merge());
    expect(group).toContainElement(replace());
    expect(merge()).toHaveAttribute('aria-checked', 'true');
    expect(replace()).toHaveAttribute('aria-checked', 'false');
  });

  it('keeps only the checked option in the tab order', () => {
    renderGroup();

    expect(merge()).toHaveAttribute('tabindex', '0');
    expect(replace()).toHaveAttribute('tabindex', '-1');

    fireEvent.click(replace());

    expect(merge()).toHaveAttribute('tabindex', '-1');
    expect(replace()).toHaveAttribute('tabindex', '0');
  });

  it('moves the selection and the focus with the arrow keys', () => {
    renderGroup();
    merge().focus();

    fireEvent.keyDown(merge(), { key: 'ArrowDown' });

    expect(replace()).toHaveAttribute('aria-checked', 'true');
    expect(replace()).toHaveFocus();

    fireEvent.keyDown(replace(), { key: 'ArrowUp' });

    expect(merge()).toHaveAttribute('aria-checked', 'true');
    expect(merge()).toHaveFocus();
  });

  it('wraps around at both ends', () => {
    renderGroup();

    fireEvent.keyDown(merge(), { key: 'ArrowLeft' });
    expect(replace()).toHaveAttribute('aria-checked', 'true');

    fireEvent.keyDown(replace(), { key: 'ArrowRight' });
    expect(merge()).toHaveAttribute('aria-checked', 'true');
  });

  it('ignores keys that are not arrows', () => {
    renderGroup();

    fireEvent.keyDown(merge(), { key: 'a' });

    expect(merge()).toHaveAttribute('aria-checked', 'true');
  });
});
