import { useRef } from 'react';

/**
 * Opens a dialog from a dropdown menu item once the menu has gone. While the
 * closing menu is still on screen it takes focus back (the pointer leaving its
 * item, its button regaining focus), and the dialog's focus trap answers by
 * re-entering with the text of its first field selected.
 *
 * Put `menuButtonRef` on the menu's trigger, `onCloseAutoFocus` on its
 * content, and wrap each item's dialog opener in `openDialog`. The menu button
 * keeps focus, without selecting anything, so the dialog returns focus there
 * when it closes.
 */
export function useMenuDialog() {
  const pendingDialog = useRef<() => void>(undefined);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const openDialog = (show: () => void) => () => {
    pendingDialog.current = show;
  };
  const onCloseAutoFocus = (event: Event) => {
    const show = pendingDialog.current;
    if (!show) return;
    pendingDialog.current = undefined;
    event.preventDefault();
    menuButtonRef.current?.focus();
    show();
  };
  return { menuButtonRef, openDialog, onCloseAutoFocus };
}
