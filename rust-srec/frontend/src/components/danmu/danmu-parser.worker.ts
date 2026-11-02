import { DanmuStreamParser, type DanmuComment } from './danmu-parser';
import {
  buildDanmuTrack,
  danmuTrackTransferables,
  type DanmuTrack,
} from './danmu-track';

/**
 * `parse` returns every comment for the list viewer; `track` returns the
 * compact overlay columns, with their buffers transferred.
 */
interface ParseRequest {
  type: 'parse' | 'track';
  url: string;
}

interface ParseError {
  type: 'error';
  message: string;
}

export type ParseResponse =
  | { type: 'success'; comments: DanmuComment[] }
  | ParseError;

export type TrackResponse = { type: 'track'; track: DanmuTrack } | ParseError;

interface WorkerContext {
  addEventListener(
    type: 'message',
    listener: (event: MessageEvent<ParseRequest>) => void,
  ): void;
  postMessage(
    message: ParseResponse | TrackResponse,
    transfer?: Transferable[],
  ): void;
}

const workerContext = self as unknown as WorkerContext;

workerContext.addEventListener('message', (event) => {
  const { type, url } = event.data;
  if (type !== 'parse' && type !== 'track') return;

  void fetchAndParse(url).then(
    (comments) => {
      if (type === 'parse') {
        workerContext.postMessage({ type: 'success', comments });
        return;
      }
      const track = buildDanmuTrack(comments);
      workerContext.postMessage(
        { type: 'track', track },
        danmuTrackTransferables(track),
      );
    },
    (error: unknown) => {
      workerContext.postMessage({
        type: 'error',
        message: error instanceof Error ? error.message : 'Unknown error',
      });
    },
  );
});

async function fetchAndParse(url: string): Promise<DanmuComment[]> {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to fetch danmu file (${response.status})`);
  }

  const parser = new DanmuStreamParser();
  if (response.body) {
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      parser.write(decoder.decode(value, { stream: true }));
    }
    parser.write(decoder.decode());
  } else {
    parser.write(await response.text());
  }

  return parser.finish();
}
