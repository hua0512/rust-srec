import { createFileRoute, useRouter } from '@tanstack/react-router';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { ChangePasswordRequestSchema } from '../../api/schemas';

import { toast } from 'sonner';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '../../components/ui/form';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { useState, useEffect } from 'react';
import { Eye, EyeOff } from 'lucide-react';

export const Route = createFileRoute('/_authed/change-password')({
  component: ChangePasswordPage,
});

import { changePassword } from '../../server/functions';

function ChangePasswordPage() {
  const { user } = Route.useRouteContext();
  const mustChangePassword = !!user?.mustChangePassword;
  const router = useRouter();
  const [showCurrentPassword, setShowCurrentPassword] = useState(false);
  const [showNewPassword, setShowNewPassword] = useState(false);
  const [showConfirmPassword, setShowConfirmPassword] = useState(false);

  // Client-side redirect check after hydration
  useEffect(() => {
    if (!user?.token) {
      router.navigate({
        to: '/login',
        search: { redirect: '/change-password' },
      });
    }
  }, [user, router]);

  const form = useForm<z.infer<typeof ChangePasswordRequestSchema>>({
    resolver: zodResolver(ChangePasswordRequestSchema),
    defaultValues: {
      current_password: '',
      new_password: '',
      confirm_password: '',
    },
  });

  const onSubmit = async (
    values: z.infer<typeof ChangePasswordRequestSchema>,
  ) => {
    try {
      await changePassword({ data: values });

      // Session is cleared after password change (all tokens revoked)
      toast.success(
        t`Password changed successfully. Please login with your new password.`,
      );

      await router.invalidate();
      router.navigate({ to: '/login', replace: true });
    } catch (error: any) {
      toast.error(error?.message || t`Failed to change password`);
    }
  };

  return (
    <div className="container relative min-h-screen flex-col items-center justify-center grid lg:max-w-none lg:grid-cols-2 lg:px-0">
      {/* Background for Mobile - Absolute positioning to cover entire screen */}
      <div className="absolute inset-0 z-0 lg:hidden overflow-hidden bg-violet-950">
        <div className="absolute inset-0 w-full h-full bg-gradient-to-br from-violet-500/90 via-fuchsia-500/90 to-blue-500/90">
          <div className="absolute top-0 -left-4 w-72 h-72 bg-purple-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob"></div>
          <div className="absolute top-0 -right-4 w-72 h-72 bg-yellow-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-2000"></div>
          <div className="absolute -bottom-8 left-20 w-72 h-72 bg-pink-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-4000"></div>
          <div className="absolute bottom-40 -right-10 w-72 h-72 bg-blue-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-2000"></div>
          <div className="absolute top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 w-96 h-96 bg-fuchsia-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-4000"></div>
        </div>
      </div>

      <div className="relative hidden w-full h-full flex-col bg-violet-950 p-10 text-white dark:border-r lg:flex overflow-hidden z-10">
        <div className="absolute inset-0 w-full h-full bg-gradient-to-br from-violet-500 via-fuchsia-500 to-blue-500 opacity-90">
          <div className="absolute top-0 -left-4 w-72 h-72 bg-purple-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob"></div>
          <div className="absolute top-0 -right-4 w-72 h-72 bg-yellow-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-2000"></div>
          <div className="absolute -bottom-8 left-20 w-72 h-72 bg-pink-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-4000"></div>
          <div className="absolute bottom-40 -right-10 w-72 h-72 bg-blue-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-2000"></div>
          <div className="absolute top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 w-96 h-96 bg-fuchsia-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-4000"></div>
        </div>
        <div className="relative z-20 flex items-center text-lg font-medium">
          <img
            src="/stream-rec.svg"
            alt="Stream-rec Logo"
            className="mr-2 h-8 w-8 brightness-0 invert"
          />
          Stream-rec
        </div>
        <div className="relative z-20 mt-auto">
          <blockquote className="space-y-2">
            <p className="text-lg">
              &ldquo;Automate your stream recordings with ease and
              reliability.&rdquo;
            </p>
          </blockquote>
        </div>
      </div>
      <div className="relative z-10 w-full h-full flex items-center justify-center p-4 lg:p-8">
        <div className="mx-auto flex w-full flex-col justify-center space-y-6 sm:w-[350px] bg-background/60 backdrop-blur-md lg:bg-transparent lg:backdrop-blur-none p-8 rounded-xl border border-white/20 shadow-xl lg:border-none lg:shadow-none lg:p-0">
          <div className="flex flex-col space-y-2 text-center">
            <h1 className="text-2xl font-semibold tracking-tight">
              {mustChangePassword ? (
                <Trans>Change Password Required</Trans>
              ) : (
                <Trans>Change Password</Trans>
              )}
            </h1>
            <p className="text-sm text-muted-foreground">
              <Trans>Please enter your new password details</Trans>
            </p>
          </div>
          <Form {...form}>
            <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
              <FormField
                control={form.control}
                name="current_password"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Current Password</Trans>
                    </FormLabel>
                    <FormControl>
                      <div className="relative">
                        <Input
                          type={showCurrentPassword ? 'text' : 'password'}
                          placeholder={t`Current password`}
                          {...field}
                          className="bg-background/50 lg:bg-background pr-10"
                        />
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="absolute right-0 top-0 h-full px-3 py-2 hover:bg-transparent"
                          onClick={() =>
                            setShowCurrentPassword(!showCurrentPassword)
                          }
                        >
                          {showCurrentPassword ? (
                            <EyeOff className="h-4 w-4 text-muted-foreground" />
                          ) : (
                            <Eye className="h-4 w-4 text-muted-foreground" />
                          )}
                          <span className="sr-only">
                            {showCurrentPassword ? (
                              <Trans>Hide password</Trans>
                            ) : (
                              <Trans>Show password</Trans>
                            )}
                          </span>
                        </Button>
                      </div>
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={form.control}
                name="new_password"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>New Password</Trans>
                    </FormLabel>
                    <FormControl>
                      <div className="relative">
                        <Input
                          type={showNewPassword ? 'text' : 'password'}
                          placeholder={t`New password (min 8 chars)`}
                          {...field}
                          className="bg-background/50 lg:bg-background pr-10"
                        />
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="absolute right-0 top-0 h-full px-3 py-2 hover:bg-transparent"
                          onClick={() => setShowNewPassword(!showNewPassword)}
                        >
                          {showNewPassword ? (
                            <EyeOff className="h-4 w-4 text-muted-foreground" />
                          ) : (
                            <Eye className="h-4 w-4 text-muted-foreground" />
                          )}
                          <span className="sr-only">
                            {showNewPassword ? (
                              <Trans>Hide password</Trans>
                            ) : (
                              <Trans>Show password</Trans>
                            )}
                          </span>
                        </Button>
                      </div>
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
                      <div className="relative">
                        <Input
                          type={showConfirmPassword ? 'text' : 'password'}
                          placeholder={t`Confirm new password`}
                          {...field}
                          className="bg-background/50 lg:bg-background pr-10"
                        />
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="absolute right-0 top-0 h-full px-3 py-2 hover:bg-transparent"
                          onClick={() =>
                            setShowConfirmPassword(!showConfirmPassword)
                          }
                        >
                          {showConfirmPassword ? (
                            <EyeOff className="h-4 w-4 text-muted-foreground" />
                          ) : (
                            <Eye className="h-4 w-4 text-muted-foreground" />
                          )}
                          <span className="sr-only">
                            {showConfirmPassword ? (
                              <Trans>Hide password</Trans>
                            ) : (
                              <Trans>Show password</Trans>
                            )}
                          </span>
                        </Button>
                      </div>
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <Button
                type="submit"
                className="w-full"
                disabled={form.formState.isSubmitting}
              >
                {form.formState.isSubmitting ? (
                  <Trans>Changing Password...</Trans>
                ) : (
                  <Trans>Change Password</Trans>
                )}
              </Button>
            </form>
          </Form>
        </div>
      </div>
    </div>
  );
}
