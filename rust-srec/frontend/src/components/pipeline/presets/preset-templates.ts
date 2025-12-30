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
  audio_extract: {
    label: msg`Audio Extract`,
    value: {
      format: 'mp3',
      bitrate: '192k',
      overwrite: true,
    },
  },
  compression: {
    label: msg`Compression (Zip)`,
    value: {
      format: 'zip',
      compression_level: 6,
      overwrite: true,
    },
  },
  copy_move: {
    label: msg`Copy/Move`,
    value: {
      operation: 'copy',
      create_dirs: true,
      overwrite: false,
    },
  },
  delete: {
    label: msg`Delete`,
    value: {
      max_retries: 3,
      retry_delay_ms: 1000,
    },
  },
  metadata: {
    label: msg`Metadata`,
    value: {
      copyright: 'My Organization',
      overwrite: true,
    },
  },
  danmaku_factory: {
    label: msg`Danmaku to ASS`,
    value: {
      overwrite: true,
      verify_output_exists: true,
      prefer_manifest: true,
      passthrough_inputs: true,
      delete_source_xml_on_success: false,
    },
  },
  ass_burnin: {
    label: msg`ASS Burn-in`,
    value: {
      video_codec: 'libx264',
      audio_codec: 'copy',
      crf: 23,
      preset: 'veryfast',
      overwrite: true,
      match_strategy: 'manifest',
      require_ass: true,
      passthrough_inputs: true,
    },
  },
};
