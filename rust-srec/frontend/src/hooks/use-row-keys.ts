import { useRef } from 'react';

/**
 * Stable React keys for the rows of a positional list edited in place.
 *
 * The rows themselves carry no identity, so their index is the only thing tying
 * one to its inputs. Keying on the index makes React reuse a deleted row's DOM
 * for its successor, which leaves the caret and the focus ring on the wrong
 * line. Keys handed out here follow the row instead, provided the caller reports
 * a deletion through `removeAt` before writing the shortened list back.
 *
 * Rows the caller appends pick up fresh keys on the next render.
 */
export function useRowKeys(length: number) {
  const state = useRef({ keys: [] as number[], nextKey: 0 });
  const { keys } = state.current;

  while (keys.length < length) {
    keys.push(state.current.nextKey++);
  }
  keys.length = length;

  return {
    keyAt: (index: number) => keys[index],
    removeAt: (index: number) => {
      keys.splice(index, 1);
    },
  };
}
