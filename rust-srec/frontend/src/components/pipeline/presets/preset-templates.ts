import { msg } from '@lingui/core/macro';

export const PRESET_TEMPLATES = {
  remux: {
    label: msg`Remux / Transcode`,
    value: {
      video_codec: 'copy',
      audio_codec: 'copy',
      format: 'mp4',
      overwrite: true,
    },
  },
  transcode_h264: {
    label: msg`Transcode H.264`,
    value: {
      video_codec: 'h264',
      audio_codec: 'aac',
      resolution: '1920x1080',
      crf: 23,
      preset: 'medium',
    },
  },
  thumbnail: {
    label: msg`Thumbnail`,
    value: {
      timestamp_secs: 10.0,
      width: 320,
      quality: 2,
    },
  },
  rclone: {
    label: msg`Rclone`,
    value: {
      operation: 'copy',
      destination_root: 'drive:/stream-recordings',
      max_retries: 3,
      args: [],
    },
  },
};
