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
});
