import { useEffect } from 'react';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Loader2 } from 'lucide-react';

import { SetPasswordSchema } from '@/api/schemas';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';

type SetPasswordValues = z.infer<typeof SetPasswordSchema>;

const EMPTY_VALUES: SetPasswordValues = {
  new_password: '',
  confirm_password: '',
};

interface SetDesktopPasswordDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The account the new password belongs to. */
  username: string | null;
  /**
   * Sets the password and turns password sign-in on. Resolves to `true` when
   * the password was set; a failure is reported by the caller.
   */
  onSubmit: (password: string) => Promise<boolean>;
}

/**
 * Asks for a first password before password sign-in is turned on, while the
 * local account still has the initial one it was created with.
 */
export function SetDesktopPasswordDialog({
  open,
  onOpenChange,
  username,
  onSubmit,
}: SetDesktopPasswordDialogProps) {
  const { i18n } = useLingui();
  const form = useForm<SetPasswordValues>({
    resolver: zodResolver(SetPasswordSchema),
    defaultValues: EMPTY_VALUES,
  });

  useEffect(() => {
    if (!open) form.reset(EMPTY_VALUES);
  }, [open, form]);

  const isSubmitting = form.formState.isSubmitting;

  const handleSubmit = async (values: SetPasswordValues) => {
    if (await onSubmit(values.new_password)) onOpenChange(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!isSubmitting) onOpenChange(next);
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            <Trans>Set a password</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Password sign-in needs a password of your own. After you set it,
              this session ends and Rust-Srec asks you to sign in with the new
              password.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <Form {...form}>
          <form
            onSubmit={form.handleSubmit(handleSubmit)}
            className="space-y-4"
          >
            {username && (
              <div className="space-y-1.5">
                <label
                  htmlFor="desktop-password-username"
                  className="block text-sm font-medium text-foreground"
                >
                  <Trans>Username</Trans>
                </label>
                {/* Also lets password managers file the new password under
                    this account. */}
                <Input
                  id="desktop-password-username"
                  name="username"
                  autoComplete="username"
                  value={username}
                  readOnly
                />
              </div>
            )}
            <FormField
              control={form.control}
              name="new_password"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>New Password</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="password"
                      placeholder={i18n._(msg`New password (min 8 chars)`)}
                      {...field}
                      autoComplete="new-password"
                      disabled={isSubmitting}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="confirm_password"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Confirm Password</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="password"
                      placeholder={i18n._(msg`Confirm new password`)}
                      {...field}
                      autoComplete="new-password"
                      disabled={isSubmitting}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={isSubmitting}
                onClick={() => onOpenChange(false)}
              >
                <Trans>Cancel</Trans>
              </Button>
              <Button type="submit" disabled={isSubmitting} className="gap-2">
                {isSubmitting && (
                  <Loader2
                    className="h-3.5 w-3.5 animate-spin"
                    aria-hidden="true"
                  />
                )}
                <Trans>Set password and require sign-in</Trans>
              </Button>
            </DialogFooter>
          </form>
        </Form>
      </DialogContent>
    </Dialog>
  );
}
