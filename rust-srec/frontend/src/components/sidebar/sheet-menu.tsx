import { Link } from '@tanstack/react-router';
import { MenuIcon } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { Button } from '@/components/ui/button';
import { Menu } from '@/components/sidebar/menu';
import {
  Sheet,
  SheetHeader,
  SheetContent,
  SheetTrigger,
  SheetTitle,
} from '@/components/ui/sheet';

export function SheetMenu() {
  return (
    <Sheet>
      <SheetTrigger className="lg:hidden" asChild>
        <Button className="h-8" variant="outline" size="icon">
          <MenuIcon size={20} />
        </Button>
      </SheetTrigger>
      <SheetContent className="w-72 px-3 h-full flex flex-col" side="left">
        <SheetHeader>
          <Button
            className="flex justify-center items-center pb-2 pt-1"
            variant="link"
            asChild
          >
            <Link to="/dashboard" className="flex items-center gap-2">
              <div className="w-6 h-6 mr-1 bg-primary/80 dark:bg-primary transition-colors [mask-image:url(/stream-rec-white.svg)] [mask-size:contain] [mask-repeat:no-repeat] [mask-position:center]" />
              <SheetTitle className="font-bold text-lg">
                <Trans>Rust-Srec</Trans>
              </SheetTitle>
            </Link>
          </Button>
        </SheetHeader>
        <Menu isOpen className="flex-1" />
      </SheetContent>
    </Sheet>
  );
}
