import { msg } from '@lingui/core/macro';

export const DEFAULT_JOB_PRESET_NAMES: Record<string, any> = {
  'preset-default-remux': msg`Remux`,
  'preset-default-remux-mkv': msg`Remux to MKV`,
  'preset-default-compress-fast': msg`Fast Compression`,
  'preset-default-compress-hq': msg`High Quality Compression`,
  'preset-default-thumbnail': msg`Thumbnail`,
  'preset-default-thumbnail-hd': msg`HD Thumbnail`,
  'preset-default-thumbnail-fullhd': msg`Full HD Thumbnail`,
  'preset-default-thumbnail-max': msg`Max Quality Thumbnail`,
  'preset-default-thumbnail-native': msg`Native Resolution Thumbnail`,
  'preset-default-audio-mp3': msg`Extract MP3`,
  'preset-default-audio-aac': msg`Extract AAC`,
  'preset-default-archive-zip': msg`ZIP Archive`,
  'preset-default-delete': msg`Delete Source`,
  'preset-default-copy': msg`Copy File`,
  'preset-default-move': msg`Move File`,
  'preset-default-upload': msg`Upload (Rclone)`,
  'preset-default-upload-delete': msg`Upload and Delete`,
  'preset-default-metadata': msg`Add Metadata`,
  'preset-default-custom-ffmpeg': msg`Custom FFmpeg`,
  'preset-remux-faststart': msg`Remux with Faststart`,
  'preset-compress-archive': msg`Archive Compression`,
  'preset-audio-mp3-hq': msg`High Quality MP3`,
  'preset-thumbnail-preview': msg`Preview Thumbnail`,
  'preset-compress-hevc-max': msg`Max HEVC Compression`,
  'preset-compress-ultrafast': msg`Ultrafast Compression`,
  'preset-remux-clean': msg`Remux and Clean`,
  'preset-default-danmu-to-ass': msg`Danmaku to ASS Subtitles`,
  'preset-default-ass-burnin': msg`ASS Subtitle Burn-in`,
};

export const PRESET_CATEGORY_NAMES: Record<string, any> = {
  remux: msg`Remux`,
  compression: msg`Compression`,
  thumbnail: msg`Thumbnail`,
  audio: msg`Audio`,
  archive: msg`Archive`,
  upload: msg`Upload`,
  cleanup: msg`Cleanup`,
  file_ops: msg`File Operations`,
  custom: msg`Custom`,
  metadata: msg`Metadata`,
  danmu: msg`Danmaku`,
  subtitle: msg`Subtitle`,
};

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
  'preset-remux-clean': msg`Remux to MP4 without re-encoding and delete the original file on success. Saves disk space.`,
  'preset-default-danmu-to-ass': msg`Convert danmu XML (Bilibili-style) into .ass subtitles using DanmakuFactory. Manifest-aware and batch-safe.`,
  'preset-default-ass-burnin': msg`Burn .ass subtitles into videos (produces *_burnin.mp4 by default). Manifest-aware and batch-safe.`,
};

export const DEFAULT_PIPELINE_PRESET_NAMES: Record<string, any> = {
  'pipeline-standard': msg`Standard`,
  'pipeline-archive': msg`Archive to Cloud`,
  'pipeline-hq-archive': msg`High Quality Archive`,
  'pipeline-podcast': msg`Podcast Extraction`,
  'pipeline-quick-share': msg`Quick Share`,
  'pipeline-space-saver': msg`Space Saver`,
  'pipeline-full': msg`Full Processing`,
  'pipeline-local-archive': msg`Local Archive`,
  'pipeline-multimedia-archive': msg`Multimedia Archive`,
  'pipeline-preview-gallery': msg`Preview Gallery`,
  'pipeline-dual-format': msg`Dual Format`,
  'pipeline-stream-archive': msg`Stream Archive`,
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
  'pipeline-multimedia-archive': msg`Full multimedia processing: Remux video, extract audio and thumbnail in parallel, then upload all.`,
  'pipeline-preview-gallery': msg`Generate multiple preview images at different timestamps for a gallery view.`,
  'pipeline-dual-format': msg`Process video and extract podcast audio in parallel, then upload both.`,
  'pipeline-stream-archive': msg`Default workflow: Remux to MP4 (deletes original), generate native-resolution thumbnail, upload both to cloud and delete local files.`,
};

export function isDefaultPreset(id: string): boolean {
  return (
    id in DEFAULT_JOB_PRESET_DESCRIPTIONS ||
    id in DEFAULT_PIPELINE_PRESET_DESCRIPTIONS
  );
}

/**
 * Get the translated name for a job preset.
 * @param preset
 * @param i18n
 */
export function getJobPresetName(
  preset: { id: string; name: string },
  i18n: any,
): string {
  if (DEFAULT_JOB_PRESET_NAMES[preset.id]) {
    return i18n._(DEFAULT_JOB_PRESET_NAMES[preset.id]);
  }
  return preset.name;
}

/**
 * Get the translated description for a job preset.
 * @param preset
 * @param i18n
 */
export function getJobPresetDescription(
  preset: { id: string; description?: string | null },
  i18n: any,
): string {
  if (DEFAULT_JOB_PRESET_DESCRIPTIONS[preset.id]) {
    return i18n._(DEFAULT_JOB_PRESET_DESCRIPTIONS[preset.id]);
  }
  return preset.description || '';
}

/**
 * Get the translated name for a pipeline preset (workflow).
 * @param workflow
 * @param i18n
 */
export function getPipelinePresetName(
  workflow: { id: string; name: string },
  i18n: any,
): string {
  if (DEFAULT_PIPELINE_PRESET_NAMES[workflow.id]) {
    return i18n._(DEFAULT_PIPELINE_PRESET_NAMES[workflow.id]);
  }
  return workflow.name;
}

/**
 * Get the translated description for a pipeline preset (workflow).
 * @param workflow
 * @param i18n
 */
export function getPipelinePresetDescription(
  workflow: { id: string; description?: string | null },
  i18n: any,
): string {
  if (DEFAULT_PIPELINE_PRESET_DESCRIPTIONS[workflow.id]) {
    return i18n._(DEFAULT_PIPELINE_PRESET_DESCRIPTIONS[workflow.id]);
  }
  return workflow.description || '';
}

/**
 * Get the translated name for a preset category.
 * @param category
 * @param i18n
 */
export function getCategoryName(category: string, i18n: any): string {
  if (PRESET_CATEGORY_NAMES[category]) {
    return i18n._(PRESET_CATEGORY_NAMES[category]);
  }
  return category;
}
