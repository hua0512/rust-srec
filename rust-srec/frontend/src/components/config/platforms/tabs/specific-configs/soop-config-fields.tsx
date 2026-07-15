import { UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Trans } from '@lingui/react/macro';
import { Key, Lock } from 'lucide-react';

interface SoopConfigFieldsProps {
  form: UseFormReturn<any>;
  fieldName: string;
}

export function SoopConfigFields({ form, fieldName }: SoopConfigFieldsProps) {
  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Key className="w-5 h-5 text-emerald-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Authentication</Trans>
          </h4>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
          <FormField
            control={form.control}
            name={`${fieldName}.username`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <div className="flex items-center gap-2 px-1">
                  <div className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>Username</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Input
                    type="text"
                    autoComplete="off"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all"
                    placeholder="example_user"
                  />
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                  <Trans>
                    SOOP account used to watch login-required (e.g. 19+)
                    broadcasts. Prefer cookies for permanently restricted
                    channels.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />

          <FormField
            control={form.control}
            name={`${fieldName}.password`}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <div className="flex items-center gap-2 px-1">
                  <div className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
                  <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                    <Trans>Password</Trans>
                  </FormLabel>
                </div>
                <FormControl>
                  <Input
                    type="password"
                    autoComplete="off"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                    placeholder="Password..."
                  />
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                  <Trans>
                    SOOP account used to watch login-required (e.g. 19+)
                    broadcasts. Prefer cookies for permanently restricted
                    channels.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>

      <section className="space-y-6">
        <div className="flex items-center gap-3 border-b border-border/40 pb-3">
          <Lock className="w-5 h-5 text-emerald-500" />
          <h4 className="text-sm font-bold uppercase tracking-[0.2em] text-foreground/80">
            <Trans>Stream Password</Trans>
          </h4>
        </div>

        <FormField
          control={form.control}
          name={`${fieldName}.stream_password`}
          render={({ field }) => (
            <FormItem className="space-y-4 max-w-xl">
              <div className="flex items-center gap-2 px-1">
                <div className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
                <FormLabel className="text-xs font-bold uppercase tracking-wider text-muted-foreground">
                  <Trans>Stream Password</Trans>
                </FormLabel>
              </div>
              <FormControl>
                <Input
                  type="password"
                  autoComplete="off"
                  {...field}
                  value={field.value || ''}
                  className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                  placeholder="Password..."
                />
              </FormControl>
              <FormDescription className="text-[11px] font-medium pt-1 px-1 text-muted-foreground/80">
                <Trans>
                  Default password for password-protected rooms (can be
                  overridden per-streamer with ?pwd= in the URL).
                </Trans>
              </FormDescription>
            </FormItem>
          )}
        />
      </section>
    </div>
  );
}
