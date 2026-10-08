import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { MessageDescriptor } from '@lingui/core';
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  HelpCircle,
  type LucideIcon,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import type { CredentialProfileDetail } from '@/api/schemas/credential-profiles';

type Health = NonNullable<CredentialProfileDetail['health']>;
export type Validity = Health['validity'];

/**
 * Consecutive failed refreshes after which a rejected account needs the user.
 * Mirrors the backend's `ATTENTION_REFRESH_FAILURES`, which decides what the
 * header's account button lists.
 */
export const ATTENTION_REFRESH_FAILURES = 3;

/** What the user has to do about an account, as the header button reports it. */
export type AttentionReason = 'login_required' | 'refresh_failing';

/**
 * Whether the account's health calls for the user: only a new login fixes an
 * invalid account, and one whose refresh keeps failing likely needs one too.
 */
export function attentionReason(
  health: Pick<Health, 'validity' | 'refresh_failure_count'> | null | undefined,
): AttentionReason | undefined {
  if (health?.validity === 'invalid') return 'login_required';
  if (
    health?.validity === 'needs_refresh' &&
    (health.refresh_failure_count ?? 0) >= ATTENTION_REFRESH_FAILURES
  )
    return 'refresh_failing';
  return undefined;
}

interface ValidityStyle {
  icon: LucideIcon;
  label: MessageDescriptor;
  /** Tinted badge surface. */
  badge: string;
  /** Status dot fill. */
  dot: string;
  /** Text that reports this state, such as the reason under a badge. */
  text: string;
}

/**
 * Green, red and muted as on the system health badges. A refresh warning is
 * amber, like the header's account button and warning notes, so an account
 * reads the same wherever it is flagged.
 */
export const VALIDITY_STYLES: Record<Validity, ValidityStyle> = {
  valid: {
    icon: CheckCircle2,
    label: msg`Valid`,
    badge:
      'border-green-500/20 bg-green-500/10 text-green-600 dark:text-green-400',
    dot: 'bg-green-500',
    text: 'text-green-600 dark:text-green-400',
  },
  needs_refresh: {
    icon: AlertTriangle,
    label: msg`Needs refresh`,
    badge:
      'border-amber-500/20 bg-amber-500/10 text-amber-600 dark:text-amber-400',
    dot: 'bg-amber-500',
    text: 'text-amber-600 dark:text-amber-400',
  },
  invalid: {
    icon: AlertCircle,
    label: msg`Login invalid`,
    badge: 'border-red-500/20 bg-red-500/10 text-red-600 dark:text-red-400',
    dot: 'bg-red-500',
    text: 'text-red-600 dark:text-red-400',
  },
  unknown: {
    icon: HelpCircle,
    label: msg`Not checked`,
    badge: 'bg-muted text-muted-foreground',
    dot: 'bg-muted-foreground/40',
    text: 'text-muted-foreground',
  },
};

/** The style of an account's validity; no health record reads as unknown. */
export function validityStyle(validity: Validity | undefined): ValidityStyle {
  return VALIDITY_STYLES[validity ?? 'unknown'];
}

/** A small coloured dot for an account's validity, labelled for screen readers. */
export function HealthDot({
  validity,
  className,
}: {
  validity: Validity | undefined;
  className?: string;
}) {
  const { i18n } = useLingui();
  const style = validityStyle(validity);
  const label = i18n._(style.label);
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className={cn(
        'inline-block size-2 shrink-0 rounded-full',
        style.dot,
        className,
      )}
    />
  );
}
