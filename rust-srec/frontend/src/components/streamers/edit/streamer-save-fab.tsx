import { AnimatePresence, motion } from 'motion/react';
import { Save, Zap } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Trans } from '@lingui/react/macro';

interface StreamerSaveFabProps {
  isDirty: boolean;
  isSaving: boolean;
  onSubmit?: () => void;
  formId?: string;
}

export function StreamerSaveFab({
  isDirty,
  isSaving,
  onSubmit,
  formId,
}: StreamerSaveFabProps) {
  return (
    <AnimatePresence>
      {(isDirty || isSaving) && (
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
