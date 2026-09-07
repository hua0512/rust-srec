import { setupI18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { describe, expect, it } from 'vitest';

import { buildCreatePipelineSchema } from '../_authed/_dashboard/pipeline/jobs/new.lazy';

// Zod keeps validation messages as plain strings, so the schema has to be
// built against the active catalog for the form errors to be translated.
const nameRequired = msg`Pipeline name is required`;

function firstIssue(i18n: ReturnType<typeof setupI18n>): string | undefined {
  const result = buildCreatePipelineSchema(i18n).safeParse({
    name: '',
    session_id: 'session-1',
    streamer_id: 'streamer-1',
    input_paths: ['/recordings/clip.mp4'],
    steps: [{}],
  });
  return result.success ? undefined : result.error.issues[0]?.message;
}

describe('buildCreatePipelineSchema', () => {
  it('uses the source message when the catalog has no translation', () => {
    const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
    expect(firstIssue(i18n)).toBe('Pipeline name is required');
  });

  it('uses the translation from the active catalog', () => {
    const i18n = setupI18n({
      locale: 'xx',
      messages: { xx: { [String(nameRequired.id)]: 'NOMBRE REQUERIDO' } },
    });
    expect(firstIssue(i18n)).toBe('NOMBRE REQUERIDO');
  });
});
