export function isPlayable(output: {
    format: string;
    file_path: string;
}): boolean {
    // Filter out thumbnails and danmu files
    if (output.format === 'THUMBNAIL' || output.format === 'DANMU_XML')
        return false;

    // Whitelist supported extensions
    const validExtensions = [
        'mp4',
        'webm',
        'ogg',
        'mp3',
        'wav',
        'mkv',
        'flv',
        'ts',
        'm3u8',
    ];
    const extension = output.file_path.split('.').pop()?.toLowerCase();

    return validExtensions.includes(extension || '');
}
