import { VALID_PROCESSORS } from '@/api/schemas';
import { getProcessorDefinition } from '../registry';

describe('getProcessorDefinition', () => {
  it.each(VALID_PROCESSORS)('has a definition for %s', (processor) => {
    expect(getProcessorDefinition(processor)).toBeDefined();
  });

  it.each([
    ['transcode', 'remux'],
    ['convert', 'remux'],
    ['upload', 'rclone'],
    ['extract_audio', 'audio_extract'],
    ['compress', 'compression'],
    ['archive', 'compression'],
    ['cleanup', 'delete'],
    ['embed_metadata', 'metadata'],
    ['command', 'execute'],
    ['danmu_to_ass', 'danmaku_factory'],
    ['danmu', 'danmaku_factory'],
    ['burn_ass', 'ass_burnin'],
    ['burn_subtitles', 'ass_burnin'],
  ])('resolves the backend alias %s to %s', (alias, processor) => {
    expect(getProcessorDefinition(alias)).toBe(
      getProcessorDefinition(processor),
    );
  });

  // The seeded "copy" and "move" presets run the copy_move processor; the
  // backend accepts neither name as a processor.
  it.each(['copy', 'move'])('has no definition for %s', (name) => {
    expect(getProcessorDefinition(name)).toBeUndefined();
  });
});
