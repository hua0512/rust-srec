type MpegtsModule = (typeof import('mpegts.js'))['default'];
type MpegtsPlayer = ReturnType<MpegtsModule['createPlayer']>;

const HAVE_CURRENT_DATA = 2;
const SEEK_RECOVERY_DELAY_MS = 3_000;
const SEEK_REBUILD_TIMEOUT_MS = 8_000;
const SEEK_TARGET_TOLERANCE_SECS = 5;

interface PendingSeek {
  targetTime: number;
  resumePlayback: boolean;
  pipelineRebuilt: boolean;
  targetFrameRendered: boolean;
}

export interface MpegtsPlaybackError {
  type: string;
  details: string;
  data: unknown;
}

export interface MpegtsPlaybackOptions {
  mediaType: 'flv' | 'mpegts';
  isLive: boolean;
  durationSecs?: number | null;
  fileSizeBytes?: number;
  onLoadingChange: (loading: boolean) => void;
  onError: (error: MpegtsPlaybackError) => void;
  onStalled: () => void;
  onWarning: (message: string, error: unknown) => void;
}

type SeekState = Pick<
  HTMLVideoElement,
  'buffered' | 'currentTime' | 'readyState'
>;

function positiveFinite(value: number | null | undefined): number | undefined {
  return value != null && Number.isFinite(value) && value > 0
    ? value
    : undefined;
}

export function hasSeekSettled(
  video: SeekState,
  targetTime: number,
  targetFrameRendered: boolean,
  playbackAdvanced: boolean,
): boolean {
  if (
    video.readyState < HAVE_CURRENT_DATA ||
    Math.abs(video.currentTime - targetTime) > SEEK_TARGET_TOLERANCE_SECS ||
    (!targetFrameRendered && !playbackAdvanced)
  ) {
    return false;
  }

  for (let index = 0; index < video.buffered.length; index += 1) {
    if (
      targetTime >= video.buffered.start(index) - SEEK_TARGET_TOLERANCE_SECS &&
      targetTime <= video.buffered.end(index) + SEEK_TARGET_TOLERANCE_SECS
    ) {
      return true;
    }
  }

  return false;
}

export class MpegtsPlaybackController {
  private player: MpegtsPlayer | null = null;
  private video: HTMLVideoElement | null = null;
  private sourceUrl: string | null = null;
  private pendingSeek: PendingSeek | null = null;
  private recoveryTimer: ReturnType<typeof setTimeout> | undefined;
  private videoFrameCallbackId: number | undefined;
  private destroyed = false;

  constructor(
    private readonly mpegts: MpegtsModule,
    private readonly options: MpegtsPlaybackOptions,
  ) {}

  attach(video: HTMLVideoElement, sourceUrl: string): void {
    if (this.destroyed) {
      throw new Error('Cannot attach a destroyed MPEG-TS playback controller');
    }

    this.video = video;
    this.sourceUrl = sourceUrl;
    this.replacePlayer();
  }

  seek(targetTime: number): boolean {
    if (
      this.options.isLive ||
      !Number.isFinite(targetTime) ||
      !this.player ||
      !this.video
    ) {
      return false;
    }

    this.clearSeekRecovery();
    this.pendingSeek = {
      targetTime,
      resumePlayback: !this.video.paused,
      pipelineRebuilt: false,
      targetFrameRendered: false,
    };
    this.options.onLoadingChange(true);

    this.watchForTargetFrame();
    this.scheduleRecovery(SEEK_RECOVERY_DELAY_MS);
    return true;
  }

  notifyMediaProgress(): boolean {
    if (!this.pendingSeek || !this.video) {
      return false;
    }

    const supportsVideoFrameCallbacks =
      typeof this.video.requestVideoFrameCallback === 'function';
    const playbackAdvanced =
      !supportsVideoFrameCallbacks &&
      this.pendingSeek.resumePlayback &&
      this.video.currentTime >= this.pendingSeek.targetTime + 0.25;
    if (
      !hasSeekSettled(
        this.video,
        this.pendingSeek.targetTime,
        this.pendingSeek.targetFrameRendered,
        playbackAdvanced,
      )
    ) {
      return false;
    }

    this.clearSeekRecovery();
    this.options.onLoadingChange(false);
    return true;
  }

  cancelSeekRecovery(): void {
    this.clearSeekRecovery();
  }

  destroy(): void {
    this.destroyed = true;
    this.clearSeekRecovery();
    this.destroyPlayer();
    this.video = null;
    this.sourceUrl = null;
  }

  private replacePlayer(seekTime?: number): MpegtsPlayer {
    if (!this.video || !this.sourceUrl) {
      throw new Error('MPEG-TS playback controller is not attached');
    }

    this.destroyPlayer();

    const durationSecs = positiveFinite(this.options.durationSecs);
    const player = this.mpegts.createPlayer(
      {
        type: this.options.mediaType,
        url: this.sourceUrl,
        isLive: this.options.isLive,
        cors: true,
        duration:
          !this.options.isLive && durationSecs != null
            ? durationSecs * 1_000
            : undefined,
        filesize: !this.options.isLive
          ? positiveFinite(this.options.fileSizeBytes)
          : undefined,
      },
      {
        isLive: this.options.isLive,
        // Starting from the preceding IDR frame avoids Chromium showing the
        // requested timestamp before its decoder can produce a picture.
        accurateSeek: false,
        rangeLoadZeroStart: !this.options.isLive,
      },
    );
    this.player = player;

    player.on(
      this.mpegts.Events.ERROR,
      (type: string, details: string, data: unknown) => {
        if (this.destroyed || player !== this.player) return;

        this.clearSeekRecovery();
        this.options.onLoadingChange(false);
        this.options.onError({ type, details, data });
      },
    );
    player.attachMediaElement(this.video);
    player.load();

    if (seekTime !== undefined) {
      player.currentTime = seekTime;
    }

    return player;
  }

  private rebuildPlayer(targetTime: number, resumePlayback: boolean): boolean {
    try {
      const player = this.replacePlayer(targetTime);
      if (resumePlayback) {
        void player.play().catch((error: unknown) => {
          if (!this.destroyed && player === this.player) {
            this.options.onWarning(
              'Unable to resume playback after seek recovery',
              error,
            );
          }
        });
      }
      if (this.pendingSeek) {
        this.pendingSeek.targetFrameRendered = false;
        this.watchForTargetFrame();
      }
      return true;
    } catch (error) {
      this.options.onWarning(
        'Unable to rebuild MPEG-TS player after a stalled seek',
        error,
      );
      return false;
    }
  }

  private scheduleRecovery(delay: number): void {
    this.clearRecoveryTimer();
    this.recoveryTimer = setTimeout(() => {
      this.recoveryTimer = undefined;
      if (this.destroyed || !this.pendingSeek || this.notifyMediaProgress()) {
        return;
      }

      if (!this.pendingSeek.pipelineRebuilt) {
        this.pendingSeek.pipelineRebuilt = true;
        this.options.onWarning('Recovering a stalled MPEG-TS seek', {
          targetTime: this.pendingSeek.targetTime,
        });
        const rebuilt = this.rebuildPlayer(
          this.pendingSeek.targetTime,
          this.pendingSeek.resumePlayback,
        );
        if (rebuilt) {
          this.scheduleRecovery(SEEK_REBUILD_TIMEOUT_MS);
          return;
        }
      }

      this.clearSeekRecovery();
      this.options.onLoadingChange(false);
      this.options.onStalled();
    }, delay);
  }

  private clearRecoveryTimer(): void {
    if (this.recoveryTimer !== undefined) {
      clearTimeout(this.recoveryTimer);
      this.recoveryTimer = undefined;
    }
  }

  private clearSeekRecovery(): void {
    this.clearRecoveryTimer();
    this.cancelVideoFrameCallback();
    this.pendingSeek = null;
  }

  private watchForTargetFrame(): void {
    const video = this.video;
    const pendingSeek = this.pendingSeek;
    if (!video?.requestVideoFrameCallback || !pendingSeek) return;

    this.cancelVideoFrameCallback();
    this.videoFrameCallbackId = video.requestVideoFrameCallback(
      (_now, metadata) => {
        this.videoFrameCallbackId = undefined;
        if (this.destroyed || this.pendingSeek !== pendingSeek) return;

        if (
          Math.abs(metadata.mediaTime - pendingSeek.targetTime) <=
          SEEK_TARGET_TOLERANCE_SECS
        ) {
          pendingSeek.targetFrameRendered = true;
          this.notifyMediaProgress();
        } else {
          this.watchForTargetFrame();
        }
      },
    );
  }

  private cancelVideoFrameCallback(): void {
    if (this.videoFrameCallbackId === undefined) return;

    this.video?.cancelVideoFrameCallback?.(this.videoFrameCallbackId);
    this.videoFrameCallbackId = undefined;
  }

  private destroyPlayer(): void {
    const player = this.player;
    if (!player) return;

    this.player = null;
    player.unload();
    player.detachMediaElement();
    player.destroy();
  }
}
