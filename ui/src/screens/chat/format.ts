/** Human-readable byte count ("512 B", "1.4 MB"). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '';
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** "m:ss" for a duration in milliseconds. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${seconds.toString().padStart(2, '0')}`;
}

/** Message kinds that the core stores as a media payload. */
export type MediaType = 'image' | 'video' | 'sticker' | 'audio' | 'voice' | 'file';

/** Maps a message type to the way its media is shown. Unknown types with a payload show as files. */
export function mediaTypeOf(type: string): MediaType {
  switch (type) {
    case 'image':
    case 'video':
    case 'sticker':
    case 'audio':
    case 'voice':
      return type;
    default:
      return 'file';
  }
}
