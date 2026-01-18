import { createFileRoute, useRouter, redirect } from '@tanstack/react-router';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { LoginRequestSchema } from '../../api/schemas';

import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { toast } from 'sonner';
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
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { useState, useCallback, memo } from 'react';
import { Eye, EyeOff } from 'lucide-react';
import { loginFn } from '@/server/functions';

export const Route = createFileRoute('/_public/login')({
  beforeLoad: ({ context }) => {
    if (context.user && context.user.mustChangePassword) {
      throw redirect({ to: '/change-password' });
    } else if (context.user) {
      throw redirect({ to: '/dashboard' });
    }
  },
  validateSearch: (search: Record<string, unknown>): { redirect?: string } => {
    return {
      redirect:
        typeof search.redirect === 'string' ? search.redirect : undefined,
    };
  },
  component: LoginComp,
});

const LoginFormSchema = LoginRequestSchema.extend({
  remember: z.boolean().default(false).optional(),
});

type LoginFormValues = z.infer<typeof LoginFormSchema>;

const LoginBackground = memo(() => (
  <div className="absolute inset-0 z-0 overflow-hidden bg-violet-950">
    <div className="absolute inset-0 w-full h-full bg-gradient-to-br from-violet-500/90 via-fuchsia-500/90 to-blue-500/90 opacity-90 lg:opacity-100">
      <div className="absolute top-0 -left-4 w-72 h-72 bg-purple-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob"></div>
      <div className="absolute top-0 -right-4 w-72 h-72 bg-yellow-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-2000"></div>
      <div className="absolute -bottom-8 left-20 w-72 h-72 bg-pink-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-4000"></div>
      <div className="absolute bottom-40 -right-10 w-72 h-72 bg-blue-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-2000"></div>
      <div className="absolute top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 w-96 h-96 bg-fuchsia-300 rounded-full mix-blend-multiply filter blur-xl opacity-70 animate-blob animation-delay-4000"></div>
    </div>
  </div>
));

LoginBackground.displayName = 'LoginBackground';

function LoginPage() {
  const { i18n } = useLingui();
  const router = useRouter();
  const search = Route.useSearch();

  const handleLoginSuccess = useCallback(
    async (mustChangePassword: boolean) => {
      await router.invalidate();

      if (mustChangePassword) {
        toast.warning(i18n._(msg`Password change required`));
        router.navigate({ to: '/change-password', replace: true });
        return;
      }

      toast.success(i18n._(msg`Logged in successfully`));

      const isValidRedirect =
        search.redirect &&
        search.redirect.startsWith('/') &&
        !search.redirect.startsWith('//') &&
        !search.redirect.includes(':');

      const safeRedirect = isValidRedirect ? search.redirect : '/dashboard';
      router.navigate({ to: safeRedirect, replace: true });
    },
    [router, search.redirect],
  );

  return (
    <div className="container relative min-h-screen flex-col items-center justify-center grid lg:max-w-none lg:grid-cols-2 lg:px-0">
      {/* Background for Mobile */}
      <div className="lg:hidden">
        <LoginBackground />
      </div>

      <div className="relative hidden w-full h-full flex-col p-10 text-white dark:border-r lg:flex overflow-hidden z-10">
        <LoginBackground />
        <div className="relative z-20 flex items-center text-lg font-medium">
          <img
            src="/stream-rec.svg"
            alt="Stream-rec Logo"
            className="mr-2 h-8 w-8 brightness-0 invert"
          />
          Rust-Srec
        </div>
        <div className="relative z-20 mt-auto">
          <blockquote className="space-y-2">
            <p className="text-lg">
              <Trans>
                Automate your stream recordings with ease and reliability.
              </Trans>
            </p>
          </blockquote>
        </div>
      </div>

      <div className="relative z-10 w-full h-full flex items-center justify-center p-4 lg:p-8">
        <div className="mx-auto flex w-full flex-col justify-center space-y-6 sm:w-[350px] bg-background/60 backdrop-blur-md lg:bg-transparent lg:backdrop-blur-none p-8 rounded-xl border border-white/20 shadow-xl lg:border-none lg:shadow-none lg:p-0">
          <div className="flex flex-col space-y-2 text-center">
            <h1 className="text-2xl font-semibold tracking-tight">
              <Trans>Welcome back</Trans>
            </h1>
            <p className="text-sm text-muted-foreground">
              <Trans>Enter your credentials to access your account</Trans>
            </p>
          </div>
          <LoginForm onSuccess={handleLoginSuccess} />
        </div>
      </div>
    </div>
  );
}

const LoginForm = memo(
  ({ onSuccess }: { onSuccess: (mustChangePassword: boolean) => void }) => {
    const { i18n } = useLingui();
    const [showPassword, setShowPassword] = useState(false);

    const form = useForm<LoginFormValues>({
      resolver: zodResolver(LoginFormSchema),
      defaultValues: {
        username: '',
        password: '',
        remember: false,
      },
    });

    const onSubmit = async (values: LoginFormValues) => {
      try {
        const response = await loginFn({
          data: {
            username: values.username,
            password: values.password,
            device_info: navigator.userAgent,
          },
        });

        onSuccess(!!response.mustChangePassword);
      } catch (error: unknown) {
        const errorMessage =
          error instanceof Error ? error.message : i18n._(msg`Login failed`);
        toast.error(errorMessage);
      }
    };

    const togglePasswordVisibility = useCallback(() => {
      setShowPassword((prev) => !prev);
    }, []);

    return (
      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-4">
          <FormField
            control={form.control}
            name="username"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Username</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder={i18n._(msg`name@example.com`)}
                    {...field}
                    className="bg-background/50 lg:bg-background"
                    autoComplete="username"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name="password"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Password</Trans>
                </FormLabel>
                <FormControl>
                  <div className="relative">
                    <Input
                      type={showPassword ? 'text' : 'password'}
                      placeholder="••••••"
                      {...field}
                      className="bg-background/50 lg:bg-background pr-10"
                      autoComplete="current-password"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="absolute right-0 top-0 h-full px-3 py-2 hover:bg-transparent"
                      onClick={togglePasswordVisibility}
                    >
                      {showPassword ? (
                        <EyeOff className="h-4 w-4 text-muted-foreground" />
                      ) : (
                        <Eye className="h-4 w-4 text-muted-foreground" />
                      )}
                      <span className="sr-only">
                        {showPassword ? (
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
          <div className="flex items-center justify-between">
            <a
              href="#"
              className="text-sm font-medium text-primary hover:text-primary/80 underline-offset-4 hover:underline"
            >
              <Trans>Restore password</Trans>
            </a>
          </div>
          <Button
            type="submit"
            className="w-full"
            disabled={form.formState.isSubmitting}
          >
            {form.formState.isSubmitting ? (
              <Trans>Signing in...</Trans>
            ) : (
              <Trans>Sign In</Trans>
            )}
          </Button>
        </form>
      </Form>
    );
  },
);

LoginForm.displayName = 'LoginForm';

function LoginComp() {
  return <LoginPage />;
}
