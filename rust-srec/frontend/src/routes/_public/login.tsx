import { createFileRoute } from '@tanstack/react-router';

export const Route = createFileRoute('/_public/login')({
  validateSearch: (search: Record<string, unknown>): { redirect?: string } => {
    return {
      redirect:
        typeof search.redirect === 'string' ? search.redirect : undefined,
    };
  },
});
