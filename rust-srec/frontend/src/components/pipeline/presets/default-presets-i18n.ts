import { msg } from '@lingui/core/macro';

export const DEFAULT_JOB_PRESET_DESCRIPTIONS: Record<string, any> = {
  'preset-default-remux': msg`Remux to MP4 without re-encoding. Fast and lossless - just changes the container format.`,
  'preset-default-remux-mkv': msg`Remux to MKV without re-encoding. Matroska supports more codecs and features.`,
  'preset-default-compress-fast': msg`Fast H.264 compression (CRF 23). Good balance of speed, quality, and file size.`,
  'preset-default-compress-hq': msg`High quality H.265/HEVC compression (CRF 22). Smaller files but slower encoding.`,
  'preset-default-thumbnail': msg`Generate a thumbnail image from the video at 10 seconds (320px width).`,
  'preset-default-thumbnail-hd': msg`Generate a high-resolution thumbnail (640px width) at 10 seconds.`,
  'preset-default-thumbnail-fullhd': msg`Generate a Full HD thumbnail (1280px width) for modern displays and video players.`,
  'preset-default-thumbnail-max': msg`Generate a maximum quality thumbnail (1920px width) preserving full 1080p detail.`,
  'preset-default-thumbnail-native': msg`Generate a thumbnail at native stream resolution (no scaling). Best quality, largest file size.`,
  'preset-default-audio-mp3': msg`Extract audio track to MP3 format (192kbps). Good for podcasts and music.`,
  'preset-default-audio-aac': msg`Extract audio track to AAC format (256kbps). High quality, widely compatible.`,
  'preset-default-archive-zip': msg`Create a ZIP archive of the file. Good for bundling with metadata.`,
  'preset-default-delete': msg`Delete the source file. Use as the last step in a pipeline to clean up.`,
  'preset-default-copy': msg`Copy the file to another location. Keeps the original file.`,
  'preset-default-move': msg`Move the file to another location. Removes the original file.`,
  'preset-default-upload': msg`Upload file to cloud storage using rclone. Configure remote in rclone config.`,
  'preset-default-upload-delete': msg`Upload file to cloud storage and delete local copy after successful upload.`,
  'preset-default-metadata': msg`Add metadata tags (title, artist, date) to the video file.`,
  'preset-default-custom-ffmpeg': msg`Run a custom FFmpeg command. Edit the args to customize.`,
  'preset-remux-faststart': msg`Remux to MP4 with faststart flag for web streaming optimization.`,
  'preset-compress-archive': msg`H.264 medium compression (CRF 23) optimized for long-term storage.`,
  'preset-audio-mp3-hq': msg`Extract audio to high-quality MP3 (320kbps) for podcast distribution.`,
  'preset-thumbnail-preview': msg`Generate a thumbnail at 30 seconds with 480px width for previews.`,
  'preset-compress-hevc-max': msg`Maximum HEVC/H.265 compression (CRF 28) for minimal file size.`,
  'preset-compress-ultrafast': msg`Ultrafast H.264 encoding (CRF 26) for quick sharing.`,
};

export const DEFAULT_PIPELINE_PRESET_DESCRIPTIONS: Record<string, any> = {
  'pipeline-standard': msg`Basic post-processing: Remux FLV to MP4 and generate a thumbnail preview.`,
  'pipeline-archive': msg`Compress video for storage, upload to cloud, then delete local file to save space.`,
  'pipeline-hq-archive': msg`Maximum quality compression with HEVC, then upload to cloud storage.`,
  'pipeline-podcast': msg`Extract high-quality audio for podcast distribution and upload.`,
  'pipeline-quick-share': msg`Fast encoding for quick sharing on social media or messaging.`,
  'pipeline-space-saver': msg`Maximum compression to minimize storage usage, then delete original.`,
  'pipeline-full': msg`Complete workflow: Remux, generate thumbnail, add metadata, and upload.`,
  'pipeline-local-archive': msg`Process locally: Remux to MP4, generate thumbnail, move to archive folder.`,
};

export function isDefaultPreset(id: string): boolean {
  return (
    id in DEFAULT_JOB_PRESET_DESCRIPTIONS ||
    id in DEFAULT_PIPELINE_PRESET_DESCRIPTIONS
  );
}
