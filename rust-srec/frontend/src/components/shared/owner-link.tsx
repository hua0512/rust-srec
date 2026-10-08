import type { ReactNode } from 'react';
import { Link } from '@tanstack/react-router';
import {
  Globe,
  LayoutTemplate,
  Settings,
  UserRound,
  type LucideIcon,
} from 'lucide-react';
import { NAME_LINK } from '@/components/shared/list-panel';

/** The icon before a name in a list of names, sized and aligned to its text. */
export const NAME_ICON = 'mr-0.5 inline h-3 w-3 align-[-2px]';

/** The style of the terms in a definition list of references. */
export const TERM = 'text-muted-foreground';

/** A configuration that a setting is made on, by the id its page is keyed by. */
export type OwnerTarget =
  | { type: 'global' }
  | { type: 'platform'; id: string }
  | { type: 'template'; id: string }
  | { type: 'streamer'; id: string };

const OWNER_ICONS: Record<OwnerTarget['type'], LucideIcon> = {
  global: Settings,
  platform: Globe,
  template: LayoutTemplate,
  streamer: UserRound,
};

/**
 * A name linking to the page where its configuration is edited, after the
 * icon of its kind unless `icon` overrides it.
 */
export function OwnerLink({
  owner,
  icon,
  title,
  children,
}: {
  owner: OwnerTarget;
  icon?: LucideIcon;
  title?: string;
  children: ReactNode;
}) {
  const Icon = icon ?? OWNER_ICONS[owner.type];
  const content = (
    <>
      <Icon className={NAME_ICON} />
      {children}
    </>
  );
  switch (owner.type) {
    case 'global':
      return (
        <Link to="/config/global" className={NAME_LINK} title={title}>
          {content}
        </Link>
      );
    case 'platform':
      return (
        <Link
          to="/config/platforms/$platformId"
          params={{ platformId: owner.id }}
          className={NAME_LINK}
          title={title}
        >
          {content}
        </Link>
      );
    case 'template':
      return (
        <Link
          to="/config/templates/$templateId"
          params={{ templateId: owner.id }}
          className={NAME_LINK}
          title={title}
        >
          {content}
        </Link>
      );
    case 'streamer':
      return (
        <Link
          to="/streamers/$id/edit"
          params={{ id: owner.id }}
          className={NAME_LINK}
          title={title}
        >
          {content}
        </Link>
      );
  }
}
