import { Link } from '@tanstack/react-router';
import { Button } from '@/components/ui/button';
import { Ghost } from 'lucide-react';

export function NotFound({ children }: { children?: any }) {
  return (
    <div className="flex flex-col items-center justify-center min-h-[50vh] space-y-4 p-4 text-center">
      <Ghost className="h-24 w-24 text-muted-foreground opacity-20" />
      <div className="space-y-2">
        <h1 className="text-4xl font-extrabold tracking-tight lg:text-5xl">
          404
        </h1>
        <div className="text-xl text-muted-foreground">
          {children || <p>The page you are looking for does not exist.</p>}
        </div>
      </div>
      <div className="flex items-center gap-2">
        <Button variant="outline" onClick={() => window.history.back()}>
          Go back
        </Button>
        <Button asChild>
          <Link to="/">Start Over</Link>
        </Button>
      </div>
    </div>
  );
}
