import { getProcessorDefinition } from '../registry';

describe('getProcessorDefinition', () => {
  it('resolves a backend alias to its processor', () => {
    expect(getProcessorDefinition('transcode')).toBe(
      getProcessorDefinition('remux'),
    );
  });

  // The seeded "copy" and "move" presets run the copy_move processor; the
  // backend accepts neither name as a processor.
  it.each(['copy', 'move'])('has no definition for %s', (name) => {
    expect(getProcessorDefinition(name)).toBeUndefined();
  });
});
