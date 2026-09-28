/**
 * A single show/hide section whose height animates open and closed.
 *
 * Pick this over `ui/accordion` or `ui/collapsible` (Radix) when the height
 * transition matters: Radix unmounts or hides closed content and animates with
 * keyframes on a measured height, so the CSS `grid-template-rows` transition
 * used here cannot apply. Use Radix Accordion for grouped, single-open lists.
 *
 * The panel stays mounted and is `inert` while closed, so its text is neither
 * focusable nor announced. Motion lives in `.rs-disclosure*` in `styles.css`.
 */
import * as React from 'react';
import { ChevronDownIcon } from 'lucide-react';

import { cn } from '@/lib/utils';

type DisclosureContextValue = {
  open: boolean;
  setOpen: (open: boolean) => void;
  contentId: string;
};

const DisclosureContext = React.createContext<DisclosureContextValue | null>(
  null,
);

function useDisclosure(component: string): DisclosureContextValue {
  const context = React.useContext(DisclosureContext);
  if (!context) {
    throw new Error(`<${component}> must be used within <Disclosure>`);
  }
  return context;
}

function Disclosure({
  open: openProp,
  defaultOpen = false,
  onOpenChange,
  className,
  children,
  ...props
}: Omit<React.ComponentProps<'div'>, 'onChange'> & {
  /** Controlled open state. Leave undefined for uncontrolled use. */
  open?: boolean;
  /** Initial state when uncontrolled. */
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const [uncontrolledOpen, setUncontrolledOpen] = React.useState(defaultOpen);
  const controlled = openProp !== undefined;
  const open = controlled ? openProp : uncontrolledOpen;
  const contentId = React.useId();

  const setOpen = React.useCallback(
    (next: boolean) => {
      if (!controlled) setUncontrolledOpen(next);
      onOpenChange?.(next);
    },
    [controlled, onOpenChange],
  );

  const value = React.useMemo(
    () => ({ open, setOpen, contentId }),
    [open, setOpen, contentId],
  );

  return (
    <DisclosureContext.Provider value={value}>
      <div
        data-slot="disclosure"
        data-state={open ? 'open' : 'closed'}
        className={cn('rs-disclosure', className)}
        {...props}
      >
        {children}
      </div>
    </DisclosureContext.Provider>
  );
}

function DisclosureTrigger({
  icon,
  className,
  children,
  onClick,
  ...props
}: React.ComponentProps<'button'> & {
  /** Optional leading icon, rendered before the label. */
  icon?: React.ReactNode;
}) {
  const { open, setOpen, contentId } = useDisclosure('DisclosureTrigger');

  return (
    <button
      type="button"
      data-slot="disclosure-trigger"
      data-state={open ? 'open' : 'closed'}
      aria-expanded={open}
      aria-controls={contentId}
      className={cn(
        'rs-disclosure-trigger flex w-full items-center gap-3 text-left font-medium outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0',
        className,
      )}
      onClick={(event) => {
        onClick?.(event);
        if (!event.defaultPrevented) setOpen(!open);
      }}
      {...props}
    >
      {icon}
      <span className="flex-1">{children}</span>
      <span
        data-slot="disclosure-chevron"
        className="rs-disclosure-chevron text-muted-foreground"
        aria-hidden="true"
      >
        <ChevronDownIcon />
      </span>
    </button>
  );
}

function DisclosureContent({
  className,
  children,
  ...props
}: React.ComponentProps<'div'>) {
  const { open, contentId } = useDisclosure('DisclosureContent');

  // The 0fr track and its clipping child must carry no padding or border, or
  // a strip stays visible when closed; caller styles go on the innermost div.
  return (
    <div
      id={contentId}
      data-slot="disclosure-content"
      data-state={open ? 'open' : 'closed'}
      className="rs-disclosure-panel"
      inert={!open}
    >
      <div className="rs-disclosure-panel-inner">
        <div className={className} {...props}>
          {children}
        </div>
      </div>
    </div>
  );
}

export { Disclosure, DisclosureTrigger, DisclosureContent };
