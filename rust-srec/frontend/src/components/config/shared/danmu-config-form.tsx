import { UseFormReturn, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Card, CardContent } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { memo, useEffect, useState } from 'react';

// Define the shapes for DanmuSamplingConfig
type SamplingStrategy = 'fixed' | 'velocity';

interface FixedSamplingConfig {
  type: 'fixed';
  interval_secs: number;
}

interface VelocitySamplingConfig {
  type: 'velocity';
  min_interval_secs: number;
  max_interval_secs: number;
  target_danmus_per_sample: number;
}

type DanmuSamplingConfig = FixedSamplingConfig | VelocitySamplingConfig;

const DEFAULT_FIXED: FixedSamplingConfig = {
  type: 'fixed',
  interval_secs: 10,
};

const DEFAULT_VELOCITY: VelocitySamplingConfig = {
  type: 'velocity',
  min_interval_secs: 1,
  max_interval_secs: 60,
  target_danmus_per_sample: 100,
};

interface DanmuConfigFormProps {
  form: UseFormReturn<any>;
  name: string;
  mode?: 'json' | 'object';
}

export const DanmuConfigForm = memo(
  ({ form, name, mode = 'json' }: DanmuConfigFormProps) => {
    const currentVal = useWatch({ control: form.control, name });
    const [config, setConfig] = useState<DanmuSamplingConfig>(DEFAULT_FIXED);

    useEffect(() => {
      if (currentVal) {
        if (mode === 'json' && typeof currentVal === 'string') {
          try {
            const parsed = JSON.parse(currentVal);
            // Validate basic shape or fallback
            if (parsed.type === 'velocity') {
              setConfig({ ...DEFAULT_VELOCITY, ...parsed });
            } else {
              setConfig({ ...DEFAULT_FIXED, ...parsed, type: 'fixed' });
            }
          } catch (e) {
            console.warn('Invalid DanmuSamplingConfig JSON', e);
          }
        } else if (typeof currentVal === 'object') {
          // Assume object has correct shape or close enough
          if (currentVal.type === 'velocity') {
            setConfig({ ...DEFAULT_VELOCITY, ...currentVal });
          } else {
            setConfig({ ...DEFAULT_FIXED, ...currentVal, type: 'fixed' });
          }
        }
      } else {
        setConfig(DEFAULT_FIXED);
      }
    }, [currentVal, mode]);

    const updateConfig = (newConfig: DanmuSamplingConfig) => {
      setConfig(newConfig);
      form.setValue(
        name,
        mode === 'json' ? JSON.stringify(newConfig) : newConfig,
        {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        },
      );
    };

    const handleStrategyChange = (value: SamplingStrategy) => {
      if (value === config.type) return;

      if (value === 'velocity') {
        updateConfig(DEFAULT_VELOCITY);
      } else {
        updateConfig(DEFAULT_FIXED);
      }
    };

    const updateField = (key: string, value: number) => {
      // @ts-ignore - straightforward updates
      updateConfig({ ...config, [key]: value });
    };

    return (
      <Card className="border-muted/40 shadow-none bg-muted/20">
        <CardContent className="p-4 space-y-4">
          <FormItem>
            <FormLabel>
              <Trans>Sampling Strategy</Trans>
            </FormLabel>
            <Select
              onValueChange={(val) =>
                handleStrategyChange(val as SamplingStrategy)
              }
              value={config.type}
            >
              <FormControl>
                <SelectTrigger className="bg-background/80">
                  <SelectValue />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectItem value="fixed">
                  <Trans>Fixed Interval</Trans>
                </SelectItem>
                <SelectItem value="velocity">
                  <Trans>Velocity Based (Dynamic)</Trans>
                </SelectItem>
              </SelectContent>
            </Select>
            <FormDescription className="text-xs">
              {config.type === 'fixed' ? (
                <Trans>Sample danmu at a constant time interval.</Trans>
              ) : (
                <Trans>
                  Adjust sampling rate based on danmu traffic volume.
                </Trans>
              )}
            </FormDescription>
          </FormItem>

          {config.type === 'fixed' && (
            <FormItem>
              <FormLabel>
                <Trans>Interval (seconds)</Trans>
              </FormLabel>
              <FormControl>
                <Input
                  type="number"
                  min={1}
                  value={config.interval_secs}
                  onChange={(e) =>
                    updateField('interval_secs', parseInt(e.target.value) || 10)
                  }
                  className="bg-background/80"
                />
              </FormControl>
              <FormDescription className="text-xs">
                <Trans>Time between sampling attempts.</Trans>
              </FormDescription>
            </FormItem>
          )}

          {config.type === 'velocity' && (
            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
              <FormItem>
                <FormLabel>
                  <Trans>Min Interval (s)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    min={1}
                    value={config.min_interval_secs}
                    onChange={(e) =>
                      updateField(
                        'min_interval_secs',
                        parseInt(e.target.value) || 1,
                      )
                    }
                    className="bg-background/80"
                  />
                </FormControl>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>Max Interval (s)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    min={1}
                    value={config.max_interval_secs}
                    onChange={(e) =>
                      updateField(
                        'max_interval_secs',
                        parseInt(e.target.value) || 60,
                      )
                    }
                    className="bg-background/80"
                  />
                </FormControl>
              </FormItem>

              <FormItem className="md:col-span-2">
                <FormLabel>
                  <Trans>Target Danmus per Sample</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    min={1}
                    value={config.target_danmus_per_sample}
                    onChange={(e) =>
                      updateField(
                        'target_danmus_per_sample',
                        parseInt(e.target.value) || 100,
                      )
                    }
                    className="bg-background/80"
                  />
                </FormControl>
                <FormDescription className="text-xs">
                  <Trans>
                    Target number of messages to capture per sample period.
                  </Trans>
                </FormDescription>
              </FormItem>
            </div>
          )}
        </CardContent>
      </Card>
    );
  },
);

DanmuConfigForm.displayName = 'DanmuConfigForm';
