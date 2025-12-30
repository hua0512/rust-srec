import { AnimatePresence, motion } from 'motion/react';
import { Save, Zap } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Trans } from '@lingui/react/macro';

import { useFormContext, useFormState, Control } from 'react-hook-form';

interface StreamerSaveFabProps {
  isSaving: boolean;
  onSubmit?: () => void;
  formId?: string;
  // Allow passing control directly when not inside a Form provider
  control?: Control<any>;
  alwaysVisible?: boolean;
}

// Inner component that uses useFormState - only renders when control is available
function SaveFabWithFormState({
  isSaving,
  onSubmit,
  formId,
  control,
  alwaysVisible,
}: StreamerSaveFabProps & { control: Control<any> }) {
  const { isDirty } = useFormState({ control });

  return (
    <AnimatePresence>
      {(isDirty || isSaving || alwaysVisible) && (
        <motion.div
          initial={{ opacity: 0, y: 50, scale: 0.9 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 50, scale: 0.9 }}
          transition={{ type: 'spring', stiffness: 300, damping: 25 }}
          className="fixed bottom-6 right-6 z-50"
        >
          <Button
            size="lg"
            type={formId ? 'submit' : 'button'}
            form={formId}
            onClick={formId ? undefined : onSubmit}
            disabled={isSaving}
            className="rounded-full shadow-lg px-6 py-6 text-base font-semibold"
          >
            {isSaving ? (
              <>
                <Zap className="mr-2 h-5 w-5 animate-spin" />
                <Trans>Saving...</Trans>
              </>
            ) : (
              <>
                <Save className="mr-2 h-5 w-5" />
                <Trans>Save Changes</Trans>
              </>
            )}
          </Button>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

export function StreamerSaveFab({
  control: propControl,
  ...props
}: StreamerSaveFabProps) {
  const formContext = useFormContext();

  // Use control from props if provided, otherwise try form context
  const control = propControl ?? formContext?.control;

  // If no control available, don't render
  if (!control) {
    return null;
  }

  return <SaveFabWithFormState {...props} control={control} />;
}
