import { ReactNode } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { Loader2, Save } from 'lucide-react';
import { Control, useFormContext, useFormState } from 'react-hook-form';
import { Button } from '@/components/ui/button';
import { usePrefersReducedMotion } from '@/hooks/use-prefers-reduced-motion';
import { cn } from '@/lib/utils';
import { Trans } from '@lingui/react/macro';

interface SaveFabProps {
  isSaving: boolean;
  /** Submits this form by id. Falls back to `onSubmit` when absent. */
  formId?: string;
  onSubmit?: () => void;
  /** Supply when rendering outside a `FormProvider`. */
  control?: Control<any>;
  /** Stay mounted while the form is clean, instead of appearing on first edit. */
  alwaysVisible?: boolean;
  /** With `alwaysVisible`, disable until something changes. */
  disabledWhenPristine?: boolean;
  /** Defaults to "Save changes". */
  label?: ReactNode;
}

/**
 * The floating save action shared by the long editing surfaces.
 *
 * One implementation so the button's size, shape and press feedback stay identical wherever it
 * appears; the pages differ only in whether it waits for a change before showing itself.
 */
function Fab({
  isSaving,
  formId,
  onSubmit,
  control,
  alwaysVisible,
  disabledWhenPristine,
  label,
}: SaveFabProps & { control: Control<any> }) {
  const { isDirty } = useFormState({ control });
  const reducedMotion = usePrefersReducedMotion();
  const visible = isDirty || isSaving || alwaysVisible;

  return (
    <AnimatePresence>
      {visible && (
        <motion.div
          initial={
            reducedMotion ? { opacity: 0 } : { opacity: 0, y: 50, scale: 0.9 }
          }
          animate={
            reducedMotion ? { opacity: 1 } : { opacity: 1, y: 0, scale: 1 }
          }
          exit={
            reducedMotion ? { opacity: 0 } : { opacity: 0, y: 50, scale: 0.9 }
          }
          transition={
            reducedMotion
              ? { duration: 0.15 }
              : { type: 'spring', stiffness: 300, damping: 25 }
          }
          className="fixed bottom-6 right-6 z-50"
        >
          <Button
            size="lg"
            type={formId ? 'submit' : 'button'}
            form={formId}
            onClick={formId ? undefined : onSubmit}
            disabled={isSaving || (disabledWhenPristine && !isDirty)}
            className={cn(
              'rounded-full border border-white/15 px-6 py-6 text-base font-semibold',
              // The button floats over scrolling content with no container to seat it, so the
              // tinted shadow and the blurred backdrop are what tie it to the page underneath.
              // Translucency is gated on `backdrop-filter` support: without the blur it would
              // just read as a washed-out fill.
              'shadow-2xl shadow-primary/30 supports-[backdrop-filter]:bg-primary/85 supports-[backdrop-filter]:backdrop-blur-xl',
              'transition-all duration-300 hover:shadow-primary/50 supports-[backdrop-filter]:hover:bg-primary/90',
              // Press feedback rather than decoration: a floating button has no surrounding
              // chrome to react, so without it a tap reads as unregistered.
              'hover:scale-[1.03] active:scale-95',
              'motion-reduce:transition-none motion-reduce:hover:scale-100 motion-reduce:active:scale-100',
            )}
          >
            {isSaving ? (
              <Loader2 className="mr-2 h-5 w-5 animate-spin" />
            ) : (
              <Save className="mr-2 h-5 w-5" />
            )}
            {isSaving ? (
              <Trans>Saving</Trans>
            ) : (
              (label ?? <Trans>Save changes</Trans>)
            )}
          </Button>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

export function SaveFab({ control: propControl, ...props }: SaveFabProps) {
  const formContext = useFormContext();
  const control = propControl ?? formContext?.control;
  if (!control) return null;
  return <Fab {...props} control={control} />;
}
