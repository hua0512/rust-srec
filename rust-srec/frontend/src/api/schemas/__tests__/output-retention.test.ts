import {
  GlobalConfigFormSchema,
  GlobalConfigSchema,
  GlobalConfigWriteSchema,
} from '../system';

describe('output retention configuration', () => {
  it('defaults old API responses to disabled, records-only retention', () => {
    const schema = GlobalConfigSchema.pick({
      output_retention_days: true,
      output_retention_delete_files: true,
    });
    expect(schema.parse({})).toEqual({
      output_retention_days: 0,
      output_retention_delete_files: false,
    });
  });

  it('does not add destructive defaults to a partial settings update', () => {
    expect(GlobalConfigWriteSchema.partial().parse({})).toEqual({});
  });

  it.each([-1, 1.5, 2147483648, '30'])(
    'rejects invalid retention days %s',
    (value) => {
      expect(
        GlobalConfigFormSchema.shape.output_retention_days.safeParse(value)
          .success,
      ).toBe(false);
      expect(
        GlobalConfigWriteSchema.shape.output_retention_days.safeParse(value)
          .success,
      ).toBe(false);
    },
  );

  it.each([false, true])(
    'preserves the explicit delete-files choice %s',
    (deleteFiles) => {
      const policy = {
        output_retention_days: 30,
        output_retention_delete_files: deleteFiles,
      };
      expect(GlobalConfigWriteSchema.partial().parse(policy)).toEqual(policy);
    },
  );
});
