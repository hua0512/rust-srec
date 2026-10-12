import type { FieldErrors, Resolver } from 'react-hook-form';

import {
  dashOverrideDisablesBothTracks,
  resolveDashSettings,
  type MesioDashConfig,
} from '@/api/schemas/engine';

interface EngineSummary {
  id: string;
  engine_type: string;
  config: unknown;
}

/**
 * IDs of the Mesio engine overrides that, merged over their engine's settings, would record
 * neither DASH video nor DASH audio. The template schema cannot see the engines, so this check
 * runs beside it.
 */
export function overridesWithoutDashTracks(
  overrides: Record<string, unknown> | null | undefined,
  engines: readonly EngineSummary[],
): string[] {
  return Object.entries(overrides ?? {}).flatMap(([id, override]) => {
    const engine = engines.find((candidate) => candidate.id === id);
    if (engine?.engine_type !== 'MESIO') return [];
    if (!override || typeof override !== 'object') return [];
    const engineDash = (
      engine.config as { dash?: Partial<MesioDashConfig> } | null
    )?.dash;
    const dash = (override as { dash?: unknown }).dash;
    return dashOverrideDisablesBothTracks(resolveDashSettings(engineDash), dash)
      ? [id]
      : [];
  });
}

/**
 * Wraps a template resolver so a save is refused when an engine override leaves no DASH track
 * to record. The error is reported on the override's audio field, beside the track settings.
 */
export function withEngineOverrideChecks<
  T extends { engines_override?: Record<string, unknown> | null },
>(
  resolver: Resolver<T>,
  getEngines: () => readonly EngineSummary[],
  message: () => string,
): Resolver<T> {
  return async (values, context, options) => {
    const result = await resolver(values, context, options);
    const ids = overridesWithoutDashTracks(
      values.engines_override,
      getEngines(),
    );
    if (ids.length === 0) return result;

    const errors = { ...result.errors } as Record<string, any>;
    const overrides = { ...errors.engines_override };
    for (const id of ids) {
      const engineErrors = { ...overrides[id] };
      engineErrors.dash = {
        ...engineErrors.dash,
        include_audio: { type: 'validate', message: message() },
      };
      overrides[id] = engineErrors;
    }
    errors.engines_override = overrides;
    return { values: {}, errors: errors as FieldErrors<T> };
  };
}
