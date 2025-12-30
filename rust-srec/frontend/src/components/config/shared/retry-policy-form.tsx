import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Card, CardContent } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { useNestedFormState } from '@/hooks/use-form-field-update';
import { SwitchCard } from '@/components/ui/switch-card';

// Define the shape of the RetryPolicy object
interface RetryPolicy {
  max_retries: number;
  initial_delay_ms: number;
  max_delay_ms: number;
  backoff_multiplier: number;
  use_jitter: boolean;
}

const DEFAULT_RETRY_POLICY: RetryPolicy = {
  max_retries: 3,
  initial_delay_ms: 1000,
  max_delay_ms: 30000,
  backoff_multiplier: 2.0,
  use_jitter: true,
};

interface RetryPolicyFormProps {
  form: UseFormReturn<any>;
  name: string; // path to the field in the form (e.g. "download_retry_policy")
  mode?: 'json' | 'object';
}

export function RetryPolicyForm({
  form,
  name,
  mode = 'json',
}: RetryPolicyFormProps) {
  // Use the custom hook to manage nested form state
  const [policy, updateField] = useNestedFormState(
    form,
    name as any,
    DEFAULT_RETRY_POLICY,
    { mode },
  );

  return (
    <Card className="border-muted/40 shadow-none bg-muted/20">
      <CardContent className="p-4 space-y-4">
        <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <FormItem>
            <FormLabel>
              <Trans>Max Retries</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                min={0}
                value={policy.max_retries}
                onChange={(e) =>
                  updateField('max_retries', parseInt(e.target.value) || 0)
                }
                className="bg-background/80"
              />
            </FormControl>
            <FormDescription className="text-xs">
              <Trans>Maximum number of retry attempts.</Trans>
            </FormDescription>
          </FormItem>

          <FormItem>
            <FormLabel>
              <Trans>Backoff Multiplier</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                step={0.1}
                min={1}
                value={policy.backoff_multiplier}
                onChange={(e) =>
                  updateField(
                    'backoff_multiplier',
                    parseFloat(e.target.value) || 1,
                  )
                }
                className="bg-background/80"
              />
            </FormControl>
            <FormDescription className="text-xs">
              <Trans>Multiplier for exponential backoff.</Trans>
            </FormDescription>
          </FormItem>

          <FormItem>
            <FormLabel>
              <Trans>Initial Delay (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                min={0}
                value={policy.initial_delay_ms}
                onChange={(e) =>
                  updateField('initial_delay_ms', parseInt(e.target.value) || 0)
                }
                className="bg-background/80"
              />
            </FormControl>
          </FormItem>

          <FormItem>
            <FormLabel>
              <Trans>Max Delay (ms)</Trans>
            </FormLabel>
            <FormControl>
              <Input
                type="number"
                min={0}
                value={policy.max_delay_ms}
                onChange={(e) =>
                  updateField('max_delay_ms', parseInt(e.target.value) || 0)
                }
                className="bg-background/80"
              />
            </FormControl>
          </FormItem>
        </div>

        <SwitchCard
          label={<Trans>Jitter</Trans>}
          description={
            <Trans>
              Add randomness to retry delays to prevent thundering herd.
            </Trans>
          }
          checked={policy.use_jitter}
          onCheckedChange={(checked) => updateField('use_jitter', checked)}
          className="bg-background/50"
        />
      </CardContent>
    </Card>
  );
}
