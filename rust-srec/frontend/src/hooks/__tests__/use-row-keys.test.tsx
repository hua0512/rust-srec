import { fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';

import { useRowKeys } from '../use-row-keys';

/**
 * Uncontrolled inputs make the reuse visible: the DOM value of a recycled node
 * keeps whatever was mounted into it, so a row rendered under its neighbour's
 * key still shows the neighbour's text.
 */
function RowList({ initial }: { initial: string[] }) {
  const [rows, setRows] = useState(initial);
  const rowKeys = useRowKeys(rows.length);

  return (
    <div>
      {rows.map((row, index) => (
        <div key={rowKeys.keyAt(index)}>
          <input defaultValue={row} />
          <button
            type="button"
            onClick={() => {
              rowKeys.removeAt(index);
              setRows(rows.filter((_, i) => i !== index));
            }}
          >
            remove {row}
          </button>
        </div>
      ))}
      <button type="button" onClick={() => setRows([...rows, 'new'])}>
        add
      </button>
    </div>
  );
}

const values = () =>
  screen
    .getAllByRole('textbox')
    .map((input) => (input as HTMLInputElement).value);

describe('useRowKeys', () => {
  it('leaves the surviving rows on their own elements after a removal', () => {
    render(<RowList initial={['a', 'b', 'c']} />);
    const second = screen.getAllByRole('textbox')[1];

    fireEvent.click(screen.getByRole('button', { name: 'remove a' }));

    expect(values()).toEqual(['b', 'c']);
    expect(screen.getAllByRole('textbox')[0]).toBe(second);
  });

  it('gives an appended row its own element', () => {
    render(<RowList initial={['a']} />);
    const first = screen.getAllByRole('textbox')[0];

    fireEvent.click(screen.getByRole('button', { name: 'add' }));

    expect(values()).toEqual(['a', 'new']);
    expect(screen.getAllByRole('textbox')[0]).toBe(first);
  });

  it('survives removing every row', () => {
    render(<RowList initial={['a', 'b']} />);

    fireEvent.click(screen.getByRole('button', { name: 'remove b' }));
    fireEvent.click(screen.getByRole('button', { name: 'remove a' }));

    expect(screen.queryAllByRole('textbox')).toHaveLength(0);
  });
});
