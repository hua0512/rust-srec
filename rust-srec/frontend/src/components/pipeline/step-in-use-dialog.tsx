import type { ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Link } from '@tanstack/react-router';
import { Workflow } from 'lucide-react';
import { z } from 'zod';
import { errorBody, errorDetails, hasErrorCode } from '@/lib/api-error';
import { formatPlatformName } from '@/lib/platform';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  DIALOG_NAME_LIMIT,
  NAME_LINK,
  NameList,
} from '@/components/shared/list-panel';
import {
  NAME_ICON,
  OwnerLink,
  TERM,
  type OwnerTarget,
} from '@/components/shared/owner-link';

const StepUserSchema = z.object({
  kind: z.enum(['global', 'platform', 'template', 'streamer', 'workflow']),
  id: z.string(),
  name: z.string(),
  removing: z.boolean(),
});

type StepUser = z.infer<typeof StepUserSchema>;

/**
 * The pipelines that still name a job preset or workflow whose deletion or
 * rename the backend refused, or `undefined` when it refused for another
 * reason.
 */
export function stepUsersFromConflict(error: unknown): StepUser[] | undefined {
  if (!hasErrorCode(errorBody(error), 'PRESET_IN_USE')) return undefined;
  const parsed = z.array(StepUserSchema).safeParse(errorDetails(error)?.users);
  return parsed.success ? parsed.data : undefined;
}

/** A refused delete or rename of the job preset or workflow called `name`. */
export interface StepInUse {
  change: 'delete' | 'rename';
  step: 'preset' | 'workflow';
  name: string;
  users: StepUser[];
}

function UserName({ user }: { user: StepUser }) {
  const { i18n } = useLingui();
  const name =
    user.kind === 'platform' ? formatPlatformName(user.name) : user.name;
  if (user.removing)
    return (
      <span
        className="min-w-0 truncate text-muted-foreground"
        title={i18n._(msg`Deleted; kept until the recordings using it finish`)}
      >
        {name}{' '}
        <span className="italic">
          <Trans>(being removed)</Trans>
        </span>
      </span>
    );
  if (user.kind === 'workflow')
    return (
      <Link
        to="/pipeline/workflows/$workflowId"
        params={{ workflowId: user.id }}
        className={NAME_LINK}
      >
        <Workflow className={NAME_ICON} />
        {name}
      </Link>
    );
  const owner: OwnerTarget =
    user.kind === 'global'
      ? { type: 'global' }
      : { type: user.kind, id: user.id };
  return <OwnerLink owner={owner}>{name}</OwnerLink>;
}

function StepUsersList({ users }: { users: StepUser[] }) {
  const groups: { kind: StepUser['kind']; term: ReactNode }[] = [
    { kind: 'global', term: <Trans>Global</Trans> },
    { kind: 'platform', term: <Trans>Platforms</Trans> },
    { kind: 'template', term: <Trans>Templates</Trans> },
    { kind: 'streamer', term: <Trans>Streamers</Trans> },
    { kind: 'workflow', term: <Trans>Workflows</Trans> },
  ];
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-xs">
      {groups.map(({ kind, term }) => {
        const items = users.filter((user) => user.kind === kind);
        if (!items.length) return null;
        return (
          <div key={kind} className="contents">
            <dt className={TERM}>{term}</dt>
            <dd className="min-w-0">
              {kind === 'global' ? (
                <OwnerLink owner={{ type: 'global' }}>
                  <Trans>Global settings</Trans>
                </OwnerLink>
              ) : (
                <NameList
                  items={items}
                  limit={DIALOG_NAME_LIMIT}
                  label={(user) => user.name}
                  render={(user) => <UserName user={user} />}
                />
              )}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}

function Explanation({ refusal }: { refusal: StepInUse }) {
  const { name } = refusal;
  if (refusal.change === 'rename')
    return (
      <Trans>
        &quot;{name}&quot; cannot be renamed while these pipelines use it,
        because they refer to it by name. Remove it from each of them first, or
        keep the current name.
      </Trans>
    );
  return (
    <Trans>
      &quot;{name}&quot; cannot be deleted while these pipelines use it. Remove
      it from each of them first.
    </Trans>
  );
}

/**
 * Lists the pipelines that keep a job preset or workflow from being deleted
 * or renamed. It has nothing to confirm, so it is a plain dialog with a close
 * button.
 */
export function StepInUseDialog({
  refusal,
  onClose,
}: {
  refusal: StepInUse | undefined;
  onClose: () => void;
}) {
  return (
    <Dialog
      open={Boolean(refusal)}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      {refusal && (
        <DialogContent className="border-border/50">
          <DialogHeader>
            <DialogTitle>
              {refusal.step === 'workflow' ? (
                <Trans>This workflow is in use</Trans>
              ) : (
                <Trans>This preset is in use</Trans>
              )}
            </DialogTitle>
            <DialogDescription asChild>
              <div className="space-y-3">
                <p>
                  <Explanation refusal={refusal} />
                </p>
                <StepUsersList users={refusal.users} />
                {refusal.users.some((user) => user.removing) && (
                  <p className="text-xs">
                    <Trans>
                      Settings being removed let go of it once the recordings
                      using them finish.
                    </Trans>
                  </p>
                )}
              </div>
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose asChild>
              <Button type="button" variant="outline">
                <Trans>Close</Trans>
              </Button>
            </DialogClose>
          </DialogFooter>
        </DialogContent>
      )}
    </Dialog>
  );
}
