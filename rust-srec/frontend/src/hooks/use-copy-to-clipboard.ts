import { useCallback, useEffect, useRef, useState } from 'react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';

const COPIED_RESET_MS = 2000;

/**
 * Copies text and reports the outcome in a toast. `copy` resolves to whether
 * the text reached the clipboard, and `copied` stays true for two seconds
 * after a successful copy, for a check-mark state on the trigger.
 */
export function useCopyToClipboard() {
  const { i18n } = useLingui();
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(resetTimer.current), []);

  const copy = useCallback(
    async (text: string, successMessage?: string): Promise<boolean> => {
      try {
        // `navigator.clipboard` is undefined outside a secure context (a
        // plain-HTTP address on the LAN); that TypeError lands here too.
        await navigator.clipboard.writeText(text);
      } catch {
        toast.error(i18n._(msg`Could not copy to the clipboard`));
        return false;
      }
      if (successMessage) toast.success(successMessage);
      setCopied(true);
      clearTimeout(resetTimer.current);
      resetTimer.current = setTimeout(() => setCopied(false), COPIED_RESET_MS);
      return true;
    },
    [i18n],
  );

  return { copy, copied };
}
