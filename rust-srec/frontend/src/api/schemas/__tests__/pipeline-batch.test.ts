import { describe, expect, it } from 'vitest';

import {
  BatchDagRequestSchema,
  BatchDeleteOutputsRequestSchema,
  BatchResponseSchema,
} from '../pipeline';

describe('BatchDagRequestSchema', () => {
  it('accepts each action the backend enum defines', () => {
    for (const type of ['cancel', 'retry', 'delete'] as const) {
      expect(
        BatchDagRequestSchema.parse({ ids: ['dag-1'], action: { type } }),
      ).toEqual({ ids: ['dag-1'], action: { type } });
    }
  });

  it('rejects an unknown action', () => {
    expect(
      BatchDagRequestSchema.safeParse({
        ids: ['dag-1'],
        action: { type: 'purge' },
      }).success,
    ).toBe(false);
  });

  // Mirrors the backend's validate_batch_ids so an invalid selection is caught
  // before it reaches the network.
  it('rejects empty, blank, duplicate and oversized ID lists', () => {
    const action = { type: 'delete' } as const;
    expect(BatchDagRequestSchema.safeParse({ ids: [], action }).success).toBe(
      false,
    );
    expect(BatchDagRequestSchema.safeParse({ ids: [''], action }).success).toBe(
      false,
    );
    expect(
      BatchDagRequestSchema.safeParse({ ids: ['dag-1', 'dag-1'], action })
        .success,
    ).toBe(false);

    const oversized = Array.from({ length: 101 }, (_, i) => `dag-${i}`);
    expect(
      BatchDagRequestSchema.safeParse({ ids: oversized, action }).success,
    ).toBe(false);
  });
});

describe('BatchDeleteOutputsRequestSchema', () => {
  it('carries the delete_file flag through', () => {
    expect(
      BatchDeleteOutputsRequestSchema.parse({
        ids: ['output-1'],
        delete_file: true,
      }),
    ).toEqual({ ids: ['output-1'], delete_file: true });
  });

  it('rejects duplicate IDs', () => {
    expect(
      BatchDeleteOutputsRequestSchema.safeParse({
        ids: ['output-1', 'output-1'],
        delete_file: false,
      }).success,
    ).toBe(false);
  });
});

describe('BatchResponseSchema', () => {
  // `code`/`error` are omitted by the backend on success, so they must stay
  // optional or every successful item would fail to parse.
  it('parses successful items without code or error', () => {
    expect(
      BatchResponseSchema.parse({
        requested: 2,
        succeeded: 1,
        failed: 1,
        results: [
          { id: 'dag-1', success: true },
          {
            id: 'dag-2',
            success: false,
            code: 'VALIDATION_ERROR',
            error: 'DAG dag-2 is already in terminal state',
          },
        ],
      }).results,
    ).toHaveLength(2);
  });
});
