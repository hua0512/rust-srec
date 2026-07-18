import { DanmuStreamParser, type DanmuComment } from './danmu-parser';

interface ParseRequest {
  type: 'parse';
  url: string;
}

export type ParseResponse =
  | { type: 'success'; comments: DanmuComment[] }
  | { type: 'error'; message: string };

interface WorkerContext {
  addEventListener(
    type: 'message',
    listener: (event: MessageEvent<ParseRequest>) => void,
  ): void;
  postMessage(message: ParseResponse): void;
}

const workerContext = self as unknown as WorkerContext;

workerContext.addEventListener('message', (event) => {
  if (event.data.type !== 'parse') return;

  void fetchAndParse(event.data.url);
});

async function fetchAndParse(url: string): Promise<void> {
  try {
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

    workerContext.postMessage({ type: 'success', comments: parser.finish() });
  } catch (error) {
    workerContext.postMessage({
      type: 'error',
      message: error instanceof Error ? error.message : 'Unknown error',
    });
  }
}
