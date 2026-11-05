import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar';
import { cn, getProxiedUrl } from '@/lib/utils';

/**
 * A streamer's picture, or the first two letters of `name` without one. It is
 * decorative: every place it appears names the streamer beside it.
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
  return (
    <Avatar className={className}>
      {image && <AvatarImage src={image} alt="" className="object-cover" />}
      <AvatarFallback
        className={cn('font-medium text-muted-foreground', fallbackClassName)}
      >
        {name?.substring(0, 2).toUpperCase() || '??'}
      </AvatarFallback>
    </Avatar>
  );
}
