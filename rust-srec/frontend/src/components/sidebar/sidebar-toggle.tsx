import * as React from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';

import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';

interface SidebarToggleProps {
  isOpen: boolean | undefined;
  setIsOpen?: () => void;
  side?: 'left' | 'right';
}

export const SidebarToggle = React.memo(function SidebarToggle({
  isOpen,
  setIsOpen,
  side = 'left',
}: SidebarToggleProps) {
  const Icon = side === 'left' ? ChevronLeft : ChevronRight;
  const placementClass = side === 'left' ? '-right-[16px]' : '-left-[16px]';

  return (
    <div
      className={cn('hidden md:block absolute top-[12px] z-20', placementClass)}
    >
      <Button
        onClick={() => setIsOpen?.()}
        className="rounded-md w-8 h-8"
        variant="outline"
        size="icon"
      >
        <Icon
          className={cn(
            'h-4 w-4 transition-transform ease-in-out duration-700',
            isOpen === false ? 'rotate-180' : 'rotate-0',
          )}
        />
      </Button>
    </div>
  );
});
