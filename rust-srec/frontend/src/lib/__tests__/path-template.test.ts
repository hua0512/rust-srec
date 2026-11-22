import { describe, expect, it } from 'vitest';

import { joinPathTemplate, splitPathTemplate } from '../path-template';

describe('splitPathTemplate / joinPathTemplate', () => {
  it.each([
    [
      '/rec/{streamer}/%Y-%m-%d',
      '/rec',
      '{streamer}/%Y-%m-%d',
      '/new/{streamer}/%Y-%m-%d',
    ],
    ['C:\\rec\\{streamer}', 'C:\\rec', '{streamer}', 'D:\\Videos\\{streamer}'],
    ['/{streamer}', '/', '{streamer}', '/new/{streamer}'],
    ['{streamer}/%Y', '', '{streamer}/%Y', '/new/{streamer}/%Y'],
    ['/srv/recordings', '/srv/recordings', '', '/new'],
  ])('%s', (path, base, suffix, rebrowsed) => {
    expect(splitPathTemplate(path)).toEqual({ base, suffix });
    const picked = path.startsWith('C:') ? 'D:\\Videos' : '/new';
    expect(joinPathTemplate(picked, suffix)).toBe(rebrowsed);
  });
});
