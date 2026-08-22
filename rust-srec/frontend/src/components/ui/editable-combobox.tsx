import {
  useEffect,
  useState,
  type ComponentPropsWithoutRef,
  type ReactNode,
} from 'react';
import { Check, ChevronDown } from 'lucide-react';

import { Button } from '@/components/ui/button';
import {
  Command,
  CommandGroup,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { Input } from '@/components/ui/input';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { cn } from '@/lib/utils';

export interface EditableComboboxOption {
  badge?: ReactNode;
  description?: ReactNode;
  inputValue: string;
  label: ReactNode;
  searchValue: string;
  value: string;
}

interface EditableComboboxProps extends Omit<
  ComponentPropsWithoutRef<'div'>,
  'onChange'
> {
  buttonLabel: string;
  displayValue: string;
  onInputBlur?: (value: string) => void;
  onInputChange: (value: string) => void;
  onOptionSelect: (option: EditableComboboxOption) => void;
  options: EditableComboboxOption[];
  placeholder?: string;
  popoverClassName?: string;
  selectedValue: string;
}

export function EditableCombobox({
  buttonLabel,
  className,
  displayValue,
  id,
  onInputBlur,
  onInputChange,
  onOptionSelect,
  options,
  placeholder,
  popoverClassName,
  selectedValue,
  'aria-describedby': ariaDescribedBy,
  'aria-invalid': ariaInvalid,
  ...containerProps
}: EditableComboboxProps) {
  const [open, setOpen] = useState(false);
  const [isEditing, setIsEditing] = useState(false);
  const [inputValue, setInputValue] = useState(displayValue);

  useEffect(() => {
    if (!isEditing) setInputValue(displayValue);
  }, [displayValue, isEditing]);

  return (
    <div
      {...containerProps}
      className={cn(
        'flex h-10 overflow-hidden rounded-xl border border-border/50 bg-background/50 transition-all focus-within:border-ring focus-within:bg-background focus-within:ring-[3px] focus-within:ring-ring/50',
        className,
      )}
    >
      <Input
        id={id}
        aria-describedby={ariaDescribedBy}
        aria-invalid={ariaInvalid}
        value={inputValue}
        onFocus={() => setIsEditing(true)}
        onBlur={() => {
          onInputBlur?.(inputValue);
          setIsEditing(false);
        }}
        onChange={(event) => {
          const nextValue = event.target.value;
          setInputValue(nextValue);
          onInputChange(nextValue);
        }}
        placeholder={placeholder}
        className="h-full flex-1 rounded-none border-0 bg-transparent shadow-none focus-visible:border-0 focus-visible:ring-0"
      />
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={buttonLabel}
            aria-expanded={open}
            className="h-full w-10 rounded-none border-l border-border/50 text-muted-foreground hover:bg-muted/50 hover:text-foreground"
          >
            <ChevronDown className="h-4 w-4" />
          </Button>
        </PopoverTrigger>
        <PopoverContent
          className={cn('w-80 p-0', popoverClassName)}
          align="end"
        >
          <Command>
            <CommandList>
              <CommandGroup>
                {options.map((option) => (
                  <CommandItem
                    key={option.value}
                    value={option.searchValue}
                    onSelect={() => {
                      onOptionSelect(option);
                      setInputValue(option.inputValue);
                      setOpen(false);
                    }}
                    className="items-start gap-3 py-2.5"
                  >
                    <Check
                      className={cn(
                        'mt-0.5 h-4 w-4',
                        selectedValue === option.value
                          ? 'opacity-100'
                          : 'opacity-0',
                      )}
                    />
                    <div className="min-w-0 flex-1">
                      <div className="text-sm font-medium">{option.label}</div>
                      {option.description ? (
                        <div className="text-xs text-muted-foreground">
                          {option.description}
                        </div>
                      ) : null}
                    </div>
                    {option.badge !== undefined && option.badge !== null ? (
                      <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
                        {option.badge}
                      </span>
                    ) : null}
                  </CommandItem>
                ))}
              </CommandGroup>
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </div>
  );
}
