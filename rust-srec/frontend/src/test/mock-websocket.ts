/**
 * A WebSocket the test drives by hand. Install it with
 * `vi.stubGlobal('WebSocket', MockWebSocket)` and reset `instances` before each
 * test; every socket the code under test opens is appended there.
 */
export class MockWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: MockWebSocket[] = [];

  readonly url: string;
  readyState = MockWebSocket.CONNECTING;
  binaryType: BinaryType = 'blob';
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  send = vi.fn();
  /** Whether the code under test called `close()`. */
  closed = false;

  constructor(url: string | URL) {
    this.url = url.toString();
    MockWebSocket.instances.push(this);
  }

  close() {
    this.closed = true;
    this.readyState = MockWebSocket.CLOSED;
  }

  emitOpen() {
    this.readyState = MockWebSocket.OPEN;
    this.onopen?.(new Event('open'));
  }

  emitClose(code?: number) {
    this.readyState = MockWebSocket.CLOSED;
    this.onclose?.(new CloseEvent('close', { code }));
  }

  emitMessage(data: string | ArrayBuffer) {
    this.onmessage?.(new MessageEvent('message', { data }));
  }

  /** Delivers `bytes` as a binary frame holding exactly those bytes. */
  emitBinary(bytes: Uint8Array) {
    this.emitMessage(
      bytes.buffer.slice(
        bytes.byteOffset,
        bytes.byteOffset + bytes.byteLength,
      ) as ArrayBuffer,
    );
  }
}
