import type { ReactNode } from 'react';
import { Link, type LinkProps } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { AlertCircle, ArrowLeft } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';

interface BackTarget {
  to: LinkProps['to'];
  label: ReactNode;
}

/** The soft colour wash behind a detail page; the page content needs `relative z-10`. */
export function AmbientBackground() {
  return (
    <div aria-hidden="true" className="pointer-events-none fixed inset-0">
      <div className="absolute top-0 left-0 -mt-20 -ml-20 h-125 w-125 rounded-full bg-primary/5 blur-[120px]" />
      <div className="absolute right-0 bottom-0 -mr-20 -mb-40 h-150 w-150 rounded-full bg-blue-500/5 blur-[120px]" />
    </div>
  );
}

/** The link above a detail page's header, back to the list it was opened from. */
export function BackLink({ to, label }: BackTarget) {
  return (
    <motion.div initial={{ opacity: 0, x: -20 }} animate={{ opacity: 1, x: 0 }}>
      <Button
        variant="ghost"
        size="sm"
        asChild
        className="group/back px-0 text-muted-foreground hover:bg-transparent hover:text-foreground"
      >
        <Link to={to} className="flex items-center">
          <ArrowLeft className="mr-2 h-4 w-4 transition-transform group-hover/back:-translate-x-1" />
          {label}
        </Link>
      </Button>
    </motion.div>
  );
}

/**
 * What a detail or edit page shows when its item could not be loaded: the
 * error's message, or `notFound` when there is none, and a way back to the
 * list.
 */
export function DetailLoadError({
  title,
  error,
  notFound,
  back,
}: {
  title: ReactNode;
  error: unknown;
  notFound: ReactNode;
  back: BackTarget;
}) {
  const message =
    error instanceof Error && error.message ? error.message : notFound;
  return (
    <div className="flex min-h-[60vh] items-center justify-center p-6">
      <Card className="w-full max-w-md border-destructive/20 bg-destructive/5 backdrop-blur-sm">
        <CardHeader className="pb-2 text-center">
          <div className="mx-auto mb-2 w-fit rounded-full bg-destructive/10 p-3">
            <AlertCircle className="h-6 w-6 text-destructive" />
          </div>
          <CardTitle className="text-xl text-destructive">{title}</CardTitle>
          <CardDescription>{message}</CardDescription>
        </CardHeader>
        <CardContent className="flex justify-center pb-6">
          <Button variant="outline" asChild>
            <Link to={back.to}>
              <ArrowLeft className="mr-2 h-4 w-4" />
              {back.label}
            </Link>
          </Button>
        </CardContent>
      </Card>
    </div>
  );
}
