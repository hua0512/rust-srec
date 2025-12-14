import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '../../ui/form';
import { Input } from '../../ui/input';
import { Switch } from '../../ui/switch';
import { Card, CardContent } from '../../ui/card';
import { Trans } from '@lingui/react/macro';
import { useEffect, useState } from 'react';

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
}

export function RetryPolicyForm({ form, name }: RetryPolicyFormProps) {
  // Local state object to manage the fields before stringifying
  const currentJsonString = form.watch(name);
  const [policy, setPolicy] = useState<RetryPolicy>(DEFAULT_RETRY_POLICY);

  // Sync local state with form's JSON string
  useEffect(() => {
    if (currentJsonString) {
      try {
        const parsed = JSON.parse(currentJsonString);
        setPolicy({ ...DEFAULT_RETRY_POLICY, ...parsed });
      } catch (e) {
        // If invalid JSON, stick to current or defaults
        console.warn('Invalid RetryPolicy JSON', e);
      }
    } else {
      // If empty (undefined/null/empty string), implies default or unset.
      // But we want to show meaningful defaults in the UI.
      setPolicy(DEFAULT_RETRY_POLICY);
    }
  }, [currentJsonString]);

  // Helper to update a single field and sync back to form
  const updateField = <K extends keyof RetryPolicy>(
    key: K,
    value: RetryPolicy[K],
  ) => {
    const newPolicy = { ...policy, [key]: value };
    setPolicy(newPolicy);
    form.setValue(name, JSON.stringify(newPolicy), {
      shouldDirty: true,
      shouldTouch: true,
      shouldValidate: true,
    });
  };

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

        <div className="flex flex-row items-center justify-between rounded-lg border p-3 bg-background/50">
          <div className="space-y-0.5">
            <FormLabel className="text-base">
              <Trans>Jitter</Trans>
            </FormLabel>
            <FormDescription className="text-xs">
              <Trans>
                Add randomness to retry delays to prevent thundering herd.
              </Trans>
            </FormDescription>
          </div>
          <Switch
            checked={policy.use_jitter}
            onCheckedChange={(checked) => updateField('use_jitter', checked)}
          />
        </div>
      </CardContent>
    </Card>
  );
}
