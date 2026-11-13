import { useState } from 'react';
import { Avatar, AvatarFallback } from '@/components/ui/avatar';
import { cn } from '@/lib/utils';
import { getProxiedUrl } from '@/lib/url';

/**
 * A streamer's picture, or the first two letters of `name` without one. It is
 * decorative: every place it appears names the streamer beside it.
 *
 * The picture is a plain lazy `<img>` over the initials rather than Radix's
 * `AvatarImage`, which preloads through `new Image()` and so fetches every
 * avatar on a page at once, off-screen ones included. Until the picture
 * loads it paints nothing and the initials show through; once it fails it is
 * removed, and once it loads the initials are, so a transparent picture does
 * not show them.
 */
export function StreamerAvatar({
  src,
  name,
  className,
  fallbackClassName,
}: {
  src: string | null | undefined;
  name: string | undefined;
  /** Size and frame of the avatar. */
  className?: string;
  /** Size of the initials, which follows the avatar's. */
  fallbackClassName?: string;
}) {
  const image = getProxiedUrl(src);
  // Keyed by the URL it describes, so a new picture starts out loading.
  const [settled, setSettled] = useState<{ src: string; loaded: boolean }>();
  const status = image && settled?.src === image ? settled : undefined;
  return (
    <Avatar className={className}>
      {!status?.loaded && (
        <AvatarFallback
          className={cn('font-medium text-muted-foreground', fallbackClassName)}
        >
          {name?.substring(0, 2).toUpperCase() || '??'}
        </AvatarFallback>
      )}
      {image && status?.loaded !== false && (
        <img
          src={image}
          alt=""
          loading="lazy"
          decoding="async"
          className="absolute inset-0 aspect-square size-full object-cover"
          onLoad={() => setSettled({ src: image, loaded: true })}
          onError={() => setSettled({ src: image, loaded: false })}
        />
      )}
    </Avatar>
  );
}
