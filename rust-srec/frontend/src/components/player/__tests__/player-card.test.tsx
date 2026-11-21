import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import { PlayerCard } from '../player-card';
import type { PlayerDanmu } from '../player-danmu';
import { usePlayerPlayback } from '../use-player-playback';
import { renderWithProviders } from '@/test/render';

vi.mock('../use-player-playback', () => ({ usePlayerPlayback: vi.fn() }));
let danmuProps: React.ComponentProps<typeof PlayerDanmu> | undefined;
vi.mock('../player-danmu', () => ({
  PlayerDanmu: (props: React.ComponentProps<typeof PlayerDanmu>) => {
    danmuProps = props;
    return null;
  },
}));
const hook = vi.mocked(usePlayerPlayback);
const reload = vi.fn();
const playback: ReturnType<typeof usePlayerPlayback> = {
  containerRef: { current: null },
  player: null,
  error: null,
  loading: false,
  reload,
  status: 'playing',
  connection: 'direct',
  statistics: null,
  supportsLivePresets: false,
};
function player(props: Partial<React.ComponentProps<typeof PlayerCard>> = {}) {
  return (
    <PlayerCard
      url="https://media.example/live.m3u8"
      sourceUrl="https://source.example/channel?token=secret"
      title="Evening stream"
      creator="Creator"
      quality="1080p"
      {...props}
    />
  );
}

beforeEach(() => {
  reload.mockReset();
  hook.mockReturnValue(playback);
});

it('shows identity, status, quality and effective connection without URL credentials', () => {
  renderWithProviders(player());
  expect(screen.getByText('Evening stream')).toBeInTheDocument();
  expect(screen.getByText(/Creator/)).toHaveTextContent(
    'source.example/channel',
  );
  expect(screen.getByRole('status')).toHaveTextContent('Playing');
  expect(screen.getByText('1080p')).toBeInTheDocument();
  expect(document.body.textContent).not.toContain('secret');
});

it('offers retry, proxy and source selection after a playback error', () => {
  hook.mockReturnValue({ ...playback, error: 'network', status: 'error' });
  renderWithProviders(player({ settingsContent: <div>Source selector</div> }));
  expect(screen.getByRole('alert')).toHaveTextContent('Check the connection');
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  expect(reload).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Try server proxy' }));
  expect(hook).toHaveBeenLastCalledWith(
    expect.objectContaining({ connectionMode: 'proxy' }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Change source' }));
  expect(screen.getByText('Source selector')).toBeInTheDocument();
});

it('shows refresh progress and a recoverable error when refreshing fails', async () => {
  let rejectRefresh: (error: Error) => void = () => {};
  const refresh = vi.fn(
    () =>
      new Promise<void>((_resolve, reject) => {
        rejectRefresh = reject;
      }),
  );
  renderWithProviders(player({ onRefreshSource: refresh }));
  fireEvent.click(screen.getByRole('button', { name: 'Player settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh stream URL' }));
  expect(screen.getByRole('status')).toHaveTextContent('Resolving stream');
  expect(
    screen.getByRole('button', { name: 'Refresh stream URL' }),
  ).toBeDisabled();
  await act(async () => rejectRefresh(new Error('secret')));
  await waitFor(() =>
    expect(screen.getByRole('alert')).toHaveTextContent(
      'could not be refreshed',
    ),
  );
  expect(screen.getByRole('alert')).not.toHaveTextContent('secret');
});

it('renews for expired live chat at most once a minute, never beside another renewal', async () => {
  let finish: () => void = () => {};
  const refresh = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  const view = renderWithProviders(
    player({ onRefreshSource: refresh, liveDanmuHandle: 'h1' }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Player settings' }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh stream URL' }));
  act(() => danmuProps?.onLiveExpired?.());
  expect(refresh).toHaveBeenCalledOnce();
  await act(async () => finish());

  // The renewal rebuilt the player with a new handle that failed at once.
  view.rerender(player({ onRefreshSource: refresh, liveDanmuHandle: 'h2' }));
  act(() => danmuProps?.onLiveExpired?.());
  expect(refresh).toHaveBeenCalledOnce();

  const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.now() + 60_000);
  act(() => danmuProps?.onLiveExpired?.());
  expect(refresh).toHaveBeenCalledTimes(2);
  clock.mockRestore();
});
