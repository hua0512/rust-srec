import { UseFormReturn } from 'react-hook-form';
import { Separator } from '@/components/ui/separator';
import { Template } from '@/api/schemas';
import { StreamerIdentityFields } from './streamer-identity-fields';
import { StreamerRecordingFields } from './streamer-recording-fields';

interface StreamerGeneralSettingsProps {
  form: UseFormReturn<any>;
  templates?: Template[];
  onAutofillName?: () => void;
  isAutofilling?: boolean;
  /**
   * Hide URL and name. The create wizard collects them in its first step, so repeating them here
   * would ask for the same two values twice.
   */
  hideIdentityFields?: boolean;
}

/** General configuration: who the streamer is, and how it is recorded. */
export function StreamerGeneralSettings({
  form,
  templates,
  onAutofillName,
  isAutofilling = false,
  hideIdentityFields = false,
}: StreamerGeneralSettingsProps) {
  return (
    <div className="space-y-5">
      {!hideIdentityFields && (
        <>
          <StreamerIdentityFields
            form={form}
            onAutofillName={onAutofillName}
            isAutofilling={isAutofilling}
          />
          <Separator />
        </>
      )}
      <StreamerRecordingFields form={form} templates={templates} />
    </div>
  );
}
