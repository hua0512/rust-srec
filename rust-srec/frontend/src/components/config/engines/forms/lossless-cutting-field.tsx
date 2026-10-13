import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { Info, TriangleAlert } from 'lucide-react';

import { Callout } from '@/components/shared/callout';
import { Badge } from '@/components/ui/badge';
import {
  Disclosure,
  DisclosureContent,
  DisclosureTrigger,
} from '@/components/ui/disclosure';
import { useEffectiveValue } from './setting-fields';
import { FeatureSwitch, type FieldSpec } from './setting-sections';

/**
 * The engine's custom-arguments field that disables chunked recording when it
 * is non-empty. Mirrors the gate in the backend FFmpeg and Streamlink engines;
 * the output format half of that gate lives in the recording configuration,
 * not in engine settings, so it can only be described here.
 */
export type LosslessCuttingArgsField = 'output_args' | 'extra_args';

export const LOSSLESS_CUTTING: FieldSpec & { kind: 'switch' } = {
  kind: 'switch',
  path: 'enable_lossless_cutting',
  label: msg`Enable lossless cutting`,
  description: msg`Split recordings without reconnecting or re-encoding. Changes apply to new recordings.`,
  fallback: false,
};

export function LosslessCuttingField({
  basePath,
  argsField,
}: {
  basePath: string;
  argsField: LosslessCuttingArgsField;
}) {
  // In an override, both follow the engine's settings where the override leaves them unset.
  const enabled = useEffectiveValue(
    `${basePath}.${LOSSLESS_CUTTING.path}`,
    LOSSLESS_CUTTING.fallback,
  );
  const args = useEffectiveValue<unknown>(`${basePath}.${argsField}`, []);
  const blockedByArgs = Array.isArray(args) && args.length > 0;

  return (
    <FeatureSwitch
      field={LOSSLESS_CUTTING}
      basePath={basePath}
      badge={
        <Badge variant="secondary">
          <Trans>Experimental</Trans>
        </Badge>
      }
    >
      {enabled && blockedByArgs && (
        <Callout
          tone="warning"
          icon={TriangleAlert}
          role="alert"
          title={<Trans>Lossless cutting will not be used</Trans>}
        >
          {argsField === 'output_args' ? (
            <Trans>
              Custom FFmpeg output arguments are set. Remove them to record with
              lossless cutting; until then this engine records normally.
            </Trans>
          ) : (
            <Trans>
              Extra Streamlink arguments are set. Remove them to record with
              lossless cutting; until then this engine records normally.
            </Trans>
          )}
        </Callout>
      )}
      {enabled && <LosslessCuttingDetails />}
    </FeatureSwitch>
  );
}

function LosslessCuttingDetails() {
  return (
    <Disclosure className="rounded-lg border bg-card text-sm text-card-foreground">
      <DisclosureTrigger
        icon={<Info aria-hidden="true" />}
        className="rounded-lg px-4 py-3 tracking-tight"
      >
        <Trans>Recording uses temporary chunks</Trans>
      </DisclosureTrigger>
      <DisclosureContent className="grid gap-1 pr-4 pb-3 pl-11 text-muted-foreground [&_p]:leading-relaxed">
        <p>
          <Trans>
            Recordings are written to temporary chunks and combined into final
            files after a cut, a size or duration limit, or recording stops.
            Final files are available only after this step finishes.
          </Trans>
        </p>
        <p>
          <Trans>
            Uses extra disk space and I/O. Temporary chunks are kept if file
            finalization fails.
          </Trans>
        </p>
        <p>
          <Trans>
            Applies only to MP4, MKV, FLV, TS, or MOV output. Recordings in
            other formats use normal recording.
          </Trans>
        </p>
      </DisclosureContent>
    </Disclosure>
  );
}
