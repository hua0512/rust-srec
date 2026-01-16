import { useEffect } from 'react';
import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { Tv } from 'lucide-react';

interface BilibiliConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function BilibiliConfigFields({
  form,
  fieldName,
}: BilibiliConfigFieldsProps) {
  useEffect(() => {
    // If current value is undefined or null, initialized to 10000 (backend default for 4K)
    const currentVal = form.getValues(`${fieldName}.quality`);
    if (currentVal === undefined || currentVal === null) {
      form.setValue(`${fieldName}.quality`, 10000);
    }
  }, [form, fieldName]);

  return (
    <div className="space-y-12">
      {/* Extraction Settings Section */}
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Tv className="w-5 h-5 text-indigo-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Extraction Settings</Trans>
          </h4>
        </div>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.quality`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <div className="flex items-center gap-2 px-1">
                  <div className="w-1.5 h-1.5 rounded-full bg-indigo-500" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>Preferred Quality (QN)</Trans>
                  </FormLabel>
                </div>
                <Select
                  onValueChange={(v) => field.onChange(parseInt(v))}
                  value={field.value?.toString() || '10000'}
                >
                  <FormControl>
                    <SelectTrigger className="bg-background/50 h-12 rounded-2xl border-border/50 focus:bg-background transition-all shadow-sm">
                      <SelectValue placeholder="Select quality" />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent className="rounded-xl border-border/50 shadow-xl">
                    <SelectItem value="30000">Dolby Vision (30000)</SelectItem>
                    <SelectItem value="20000">HDR (20000)</SelectItem>
                    <SelectItem value="10000">4K (10000)</SelectItem>
                    <SelectItem value="127">8K (127)</SelectItem>
                    <SelectItem value="125">HDR (125)</SelectItem>
                    <SelectItem value="120">4K (120)</SelectItem>
                    <SelectItem value="116">1080P60 (116)</SelectItem>
                    <SelectItem value="112">1080P+ (112)</SelectItem>
                    <SelectItem value="80">1080P (80)</SelectItem>
                    <SelectItem value="64">720P (64)</SelectItem>
                    <SelectItem value="32">480P (32)</SelectItem>
                  </SelectContent>
                </Select>
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                  <Trans>
                    Select the highest quality level you want to attempt
                    capturing.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>
    </div>
  );
}
