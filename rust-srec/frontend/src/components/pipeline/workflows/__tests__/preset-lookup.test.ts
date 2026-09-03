import { findPresetByName } from '../preset-lookup';

const presets = [
  // `/job/presets?search=` matches descriptions too, so a response can lead with a preset that
  // only mentions the searched name.
  {
    name: 'delete_source',
    processor: 'delete',
    description:
      'Safe after an Upload step; after a remux it deletes the result.',
  },
  { name: 'remux', processor: 'remux', description: 'Remux to mp4' },
];

describe('findPresetByName', () => {
  it('returns the preset whose name matches exactly', () => {
    expect(findPresetByName(presets, 'remux')?.processor).toBe('remux');
  });

  it('does not fall back to another row when no name matches', () => {
    expect(findPresetByName(presets, 'upload')).toBeNull();
  });

  it('returns null for a missing list or name', () => {
    expect(findPresetByName(undefined, 'remux')).toBeNull();
    expect(findPresetByName(presets, null)).toBeNull();
    expect(findPresetByName(presets, '')).toBeNull();
  });
});
