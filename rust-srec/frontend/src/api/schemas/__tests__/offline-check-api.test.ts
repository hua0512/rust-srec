import { describe, expect, it } from 'vitest';

import { PlatformConfigFormSchema, PlatformConfigSchema } from '../platform';
import {
  StreamerSpecificConfigFormSchema,
  StreamerSpecificConfigSchema,
} from '../streamer';
import {
  GlobalConfigFormSchema,
  GlobalConfigSchema,
  GlobalConfigWriteSchema,
} from '../system';
import {
  CreateTemplateRequestSchema,
  TemplateSchema,
  UpdateTemplateRequestSchema,
} from '../template';

const offlineCheckSchemas = [
  ['global API response', GlobalConfigSchema],
  ['global form', GlobalConfigFormSchema],
  ['global API request', GlobalConfigWriteSchema],
  ['platform API response', PlatformConfigSchema],
  ['platform form', PlatformConfigFormSchema],
  ['template API response', TemplateSchema],
  ['template create request', CreateTemplateRequestSchema],
  ['template update request', UpdateTemplateRequestSchema],
  ['streamer API config', StreamerSpecificConfigSchema],
  ['streamer form config', StreamerSpecificConfigFormSchema],
] as const;

describe('offline-check API field names', () => {
  it.each(offlineCheckSchemas)(
    '%s uses only the current field names',
    (_, schema) => {
      const keys = new Set(schema.keyof().options);

      expect(keys).toContain('offline_check_count');
      expect(keys).toContain('offline_check_delay_ms');
      expect(keys).not.toContain('effective_offline_check_count');
      expect(keys).not.toContain('effective_offline_check_delay_ms');
    },
  );
});
