import { createFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { useLingui } from '@lingui/react';
import { Check } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Trans } from '@lingui/react/macro';
import { Card } from '@/components/ui/card';
import { dynamicActivate, Locale } from '@/integrations/lingui/i18n';
import { updateLocale } from '@/server/functions/locale';
import { useRouter } from '@tanstack/react-router';

export const Route = createFileRoute('/_authed/_dashboard/config/language')({
  component: LanguageSettings,
});

function LanguageSettings() {
  const { i18n } = useLingui();
  const router = useRouter();

  const locales: Array<{
    code: Locale;
    name: string;
    nativeName: string;
    flag: string;
    description: string;
  }> = [
    {
      code: 'en',
      name: 'English',
      nativeName: 'English',
      flag: '🇺🇸',
      description: 'English (International)',
    },
    {
      code: 'zh-CN',
      name: 'Chinese',
      nativeName: '简体中文',
      flag: '🇨🇳',
      description: 'Chinese Simplified',
    },
  ];

  const changeLocale = async (locale: Locale) => {
    await updateLocale({ data: locale });
    await dynamicActivate(i18n, locale);
    await router.invalidate();
  };

  return (
    <div className="flex flex-col gap-6 sm:gap-8 min-h-[calc(100vh-8rem)]">
      <motion.div
        initial={{ opacity: 0, y: -20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.4 }}
        className="max-w-2xl px-1"
      >
        <div className="grid gap-4 sm:gap-6">
          {locales.map((locale, index) => {
            const isActive = i18n.locale === locale.code;
            return (
              <motion.div
                key={locale.code}
                initial={{ opacity: 0, x: -20 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ duration: 0.4, delay: index * 0.1 }}
                onClick={() => changeLocale(locale.code)}
              >
                <Card
                  className={cn(
                    'group relative overflow-hidden cursor-pointer transition-all duration-300 border-2',
                    'hover:shadow-2xl hover:shadow-primary/5 hover:-translate-y-1',
                    'backdrop-blur-xl bg-background/40 dark:bg-card/30',
                    isActive
                      ? 'border-primary ring-1 ring-primary/20 bg-primary/[0.03] dark:bg-primary/[0.05]'
                      : 'border-white/5 hover:border-primary/30',
                  )}
                >
                  <div
                    className={cn(
                      'absolute inset-0 bg-gradient-to-br transition-opacity duration-500',
                      isActive
                        ? 'from-primary/10 via-transparent to-transparent opacity-100'
                        : 'from-primary/5 via-transparent to-transparent opacity-0 group-hover:opacity-100',
                    )}
                  />

                  <div className="relative z-10 p-3 sm:p-6 flex items-center gap-3 sm:gap-6">
                    <div
                      className={cn(
                        'w-10 h-10 sm:w-16 sm:h-16 rounded-xl sm:rounded-2xl flex items-center justify-center text-xl sm:text-3xl shadow-inner transition-transform duration-500 group-hover:scale-110 shrink-0',
                        isActive
                          ? 'bg-primary/20'
                          : 'bg-muted/50 group-hover:bg-primary/10',
                      )}
                    >
                      {locale.flag}
                    </div>

                    <div className="flex-1 space-y-0.5 sm:space-y-1 min-w-0">
                      <div className="flex items-center gap-2">
                        <h3 className="text-base sm:text-xl font-bold tracking-tight truncate">
                          {locale.nativeName}
                        </h3>
                        {isActive && (
                          <div className="px-1.5 py-0.5 rounded-full bg-primary/20 text-primary text-[8px] sm:text-[10px] font-bold uppercase tracking-wider shrink-0">
                            <Trans>Active</Trans>
                          </div>
                        )}
                      </div>
                      <p className="text-muted-foreground text-[10px] sm:text-sm font-medium truncate">
                        {locale.name} — {locale.description}
                      </p>
                    </div>

                    <div
                      className={cn(
                        'w-7 h-7 sm:w-10 sm:h-10 rounded-full flex items-center justify-center border-2 transition-all duration-300 shrink-0',
                        isActive
                          ? 'bg-primary border-primary text-primary-foreground scale-100 opacity-100'
                          : 'border-muted group-hover:border-primary/50 text-transparent scale-75 opacity-0 group-hover:opacity-50',
                      )}
                    >
                      <Check className="w-3 h-3 sm:w-5 sm:h-5" />
                    </div>
                  </div>
                </Card>
              </motion.div>
            );
          })}
        </div>
      </motion.div>

      {/* Decorative background element */}
      <div className="fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 -z-10 w-[500px] h-[500px] bg-primary/5 rounded-full blur-[120px] pointer-events-none animate-pulse" />
    </div>
  );
}
