import type { MediaOutput } from '@/api/schemas/system';
import type { SessionSegment } from '@/api/schemas/session';
import {
  buildTimelineGroups,
  classifyTimelineBoundary,
} from '../recordings-tab';

function output(
  filePath: string,
  createdAt: string,
  format = 'VIDEO',
): MediaOutput {
  return {
    id: filePath,
    session_id: 'session-1',
    streamer_id: 'streamer-1',
    file_path: filePath,
    file_size_bytes: 100,
    duration_secs: 60,
    format,
    created_at: createdAt,
  };
}

function segment(
  filePath: string,
  index: number,
  createdAt: string,
  completedAt: string,
  splitReasonCode?: string,
): SessionSegment {
  return {
    id: `segment-${index}`,
    session_id: 'session-1',
    segment_index: index,
    file_path: filePath,
    duration_secs: 60,
    size_bytes: 100,
    split_reason_code: splitReasonCode,
    created_at: createdAt,
    completed_at: completedAt,
    persisted_at: completedAt,
  };
}

describe('recordings timeline boundaries', () => {
  it('associates a boundary with the preceding segment split reason', () => {
    const outputs = [
      output('/recordings/first.m4s', '2026-01-01T00:01:00Z'),
      output('/recordings/second.m4s', '2026-01-01T00:02:00Z'),
      output('/recordings/third.m4s', '2026-01-01T00:03:00Z'),
    ];
    const segments = [
      segment(
        '/recordings/first.m4s',
        0,
        '2026-01-01T00:00:00Z',
        '2026-01-01T00:01:00Z',
        'size_limit',
      ),
      segment(
        '/recordings/second.m4s',
        1,
        '2026-01-01T00:01:00Z',
        '2026-01-01T00:02:00Z',
        'discontinuity',
      ),
      segment(
        '/recordings/third.m4s',
        2,
        '2026-01-01T00:02:00Z',
        '2026-01-01T00:03:00Z',
      ),
    ];

    const groups = buildTimelineGroups(outputs, segments);

    expect(groups[0].boundaryReasonBefore).toBeUndefined();
    expect(groups[1].boundaryReasonBefore?.code).toBe('size_limit');
    expect(groups[2].boundaryReasonBefore?.code).toBe('discontinuity');
  });

  it('distinguishes discontinuities from time gaps', () => {
    expect(classifyTimelineBoundary({ code: 'discontinuity' }, 0)).toBe(
      'discontinuity',
    );
    expect(classifyTimelineBoundary({ code: 'discontinuity' }, 6)).toBe(
      'break',
    );
    expect(classifyTimelineBoundary({ code: 'size_limit' }, 0)).toBe(
      'lossless_split',
    );
  });

  it('groups historical Windows verbatim and regular drive paths', () => {
    const verbatimStem = String.raw`\\?\G:\recordings\stream`;
    const regularStem = String.raw`G:\recordings\stream`;
    const outputs = [
      output(`${verbatimStem}.flv`, '2026-01-01T00:01:00Z'),
      output(`${verbatimStem}.jpg`, '2026-01-01T00:00:01Z', 'THUMBNAIL'),
      output(`${regularStem}.xml`, '2026-01-01T00:00:59Z', 'DANMU_XML'),
    ];
    const segments = [
      segment(
        `${verbatimStem}.flv`,
        0,
        '2026-01-01T00:00:00Z',
        '2026-01-01T00:01:00Z',
        'size_limit',
      ),
    ];

    const groups = buildTimelineGroups(outputs, segments);

    expect(groups).toHaveLength(1);
    expect(groups[0].id).toBe('G:/recordings/stream');
    expect(groups[0].baseName).toBe('stream');
    expect(groups[0].outputs).toHaveLength(3);
    expect(groups[0].splitReason?.code).toBe('size_limit');
  });

  it('groups historical Windows verbatim and regular UNC paths', () => {
    const outputs = [
      output(
        String.raw`\\?\UNC\server\share\stream.flv`,
        '2026-01-01T00:01:00Z',
      ),
      output(
        String.raw`\\server\share\stream.xml`,
        '2026-01-01T00:00:59Z',
        'DANMU_XML',
      ),
    ];

    const groups = buildTimelineGroups(outputs);

    expect(groups).toHaveLength(1);
    expect(groups[0].id).toBe('//server/share/stream');
    expect(groups[0].outputs).toHaveLength(2);
  });
});
