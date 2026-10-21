import { expect, it } from 'vitest';
import { truncateChars } from '../char-count';

// Two UTF-16 code units, one character for the backend.
const EMOJI = '😀';

it('cuts to a character limit, counting an emoji as one character', () => {
  expect(truncateChars(EMOJI.repeat(100), 100)).toBe(EMOJI.repeat(100));
  expect(truncateChars(`a${EMOJI.repeat(3)}`, 3)).toBe(`a${EMOJI.repeat(2)}`);
  expect(truncateChars('abcd', 3)).toBe('abc');
});
