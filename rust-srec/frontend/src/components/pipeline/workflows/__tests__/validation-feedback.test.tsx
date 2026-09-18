import { setupI18n } from '@lingui/core';
import { toast } from 'sonner';
import {
  type DagValidationResult,
  reportValidation,
  validateBeforeSave,
} from '../validation-feedback';

vi.mock('sonner', () => ({
  toast: { error: vi.fn(), warning: vi.fn(), success: vi.fn() },
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });
const clean: DagValidationResult = {
  valid: true,
  errors: [],
  warnings: [],
  max_depth: 1,
};
const dag = { name: 'archive', steps: [] };

beforeEach(() => {
  vi.clearAllMocks();
});

describe('reportValidation', () => {
  it('blocks on errors and lists the warnings with them', () => {
    const result = {
      ...clean,
      valid: false,
      errors: ['bad'],
      warnings: ['meh'],
    };
    expect(reportValidation(i18n, result, { announceSuccess: true })).toBe(
      false,
    );
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.warning).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('warns but passes when only warnings remain', () => {
    const result = { ...clean, warnings: ['meh'] };
    expect(reportValidation(i18n, result, { announceSuccess: true })).toBe(
      true,
    );
    expect(toast.warning).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('announces a clean result only when asked', () => {
    expect(reportValidation(i18n, clean, { announceSuccess: false })).toBe(
      true,
    );
    expect(toast.success).not.toHaveBeenCalled();
    expect(reportValidation(i18n, clean, { announceSuccess: true })).toBe(true);
    expect(toast.success).toHaveBeenCalledTimes(1);
  });
});

describe('validateBeforeSave', () => {
  it('stops the save on errors and continues on warnings', async () => {
    const failing = vi.fn(async () => ({
      ...clean,
      valid: false,
      errors: ['bad'],
    }));
    expect(await validateBeforeSave(i18n, dag, failing)).toBe(false);
    expect(failing).toHaveBeenCalledWith(dag);
    expect(toast.error).toHaveBeenCalledTimes(1);

    const warning = vi.fn(async () => ({ ...clean, warnings: ['meh'] }));
    expect(await validateBeforeSave(i18n, dag, warning)).toBe(true);
    expect(toast.warning).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('lets the save proceed when the validation service is unavailable', async () => {
    const consoleError = vi
      .spyOn(console, 'error')
      .mockImplementation(() => {});
    const down = vi.fn(async () => {
      throw new Error('down');
    });
    expect(await validateBeforeSave(i18n, dag, down)).toBe(true);
    expect(toast.error).not.toHaveBeenCalled();
    expect(consoleError).toHaveBeenCalledTimes(1);
    consoleError.mockRestore();
  });
});
