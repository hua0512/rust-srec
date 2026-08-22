import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Zap, Activity } from 'lucide-react';
import { HuyaPlatformValues } from '@/api/schemas/platform-configs';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';

const HUYA_PLATFORM_LABELS: Record<
  (typeof HuyaPlatformValues)[number],
  string
> = {
  huya_pc_exe: 'PC Client',
  huya_adr: 'Android',
  huya_ios: 'iOS',
  tv_huya_nftv: 'TV',
  huya_webh5: 'Web H5',
  tars_mp: 'Mini Program',
  tars_mobile: 'WAP / Mobile',
  huya_liveshareh5: 'Live Share H5',
  random: 'Random',
};

interface HuyaConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function HuyaConfigFields({ form, fieldName }: HuyaConfigFieldsProps) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-12">
      {/* Protocol Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Zap} accent="indigo">
          <Trans>Protocol Settings</Trans>
        </ConfigSectionHeading>

        <div className="space-y-6">
          <FormField
            control={form.control}
            name={`${fieldName}.api_mode`}
            render={({ field }) => (
              <FormItem>
                <ConfigFieldLabel accent="indigo" className="mb-3">
                  <Trans>Extraction API Mode</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value || 'WEB'}
                  >
                    <SelectTrigger className="bg-background/50 h-11 rounded-xl border-border/50 focus:bg-background transition-all shadow-sm">
                      <SelectValue placeholder={i18n._(msg`Select API Mode`)} />
                    </SelectTrigger>
                    <SelectContent className="rounded-xl border-border/50 shadow-xl">
                      <SelectItem value="WEB">
                        <Trans>WEB</Trans>{' '}
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Default</Trans>)
                        </span>
                      </SelectItem>
                      <SelectItem value="MP">
                        <Trans>MP</Trans>
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Numeric Room IDs Only</Trans>)
                        </span>
                      </SelectItem>
                      <SelectItem value="WUP">
                        <Trans>WUP</Trans>{' '}
                        <span className="text-muted-foreground ml-2 text-xs">
                          (<Trans>Numeric Room IDs Only</Trans>)
                        </span>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    API protocol to use for live stream extraction. WUP is the
                    standard protocol for the PC app. Note that WUP and MP only
                    work with numeric room IDs.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={`${fieldName}.platform`}
            render={({ field }) => (
              <FormItem>
                <ConfigFieldLabel accent="indigo" className="mb-3">
                  <Trans>Client Type (ctype)</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Select
                    onValueChange={field.onChange}
                    value={field.value || 'huya_pc_exe'}
                  >
                    <SelectTrigger className="bg-background/50 h-11 rounded-xl border-border/50 focus:bg-background transition-all shadow-sm">
                      <SelectValue
                        placeholder={i18n._(msg`Select Client Type`)}
                      />
                    </SelectTrigger>
                    <SelectContent className="rounded-xl border-border/50 shadow-xl">
                      {HuyaPlatformValues.map((value) => (
                        <SelectItem key={value} value={value}>
                          {HUYA_PLATFORM_LABELS[value]}
                          {value === 'huya_pc_exe' && (
                            <span className="text-muted-foreground ml-2 text-xs">
                              (<Trans>Default</Trans>)
                            </span>
                          )}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    Client platform type used for stream authentication signing.
                    Different platforms use different signing methods.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>

      {/* Quality Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Activity} accent="indigo">
          <Trans>Quality Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.force_origin_quality`}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-2xl border bg-muted/5 p-5 transition-all hover:bg-muted/10 border-border/50">
                <div className="space-y-1.5 pr-4">
                  <FormLabel className="text-sm font-bold text-foreground">
                    <Trans>Force Origin Quality</Trans>
                  </FormLabel>
                  <FormDescription className="text-xs leading-relaxed font-medium">
                    <Trans>
                      Force requesting the highest origin quality available
                      (Direct stream).
                    </Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={!!field.value}
                    onCheckedChange={field.onChange}
                  />
                </FormControl>
              </FormItem>
            )}
          />
        </div>
      </section>
    </div>
  );
}
