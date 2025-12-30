import * as React from 'react';
import { X } from 'lucide-react';
import { Badge } from './badge';
import { Input } from './input';
import { cn } from '../../lib/utils';

interface TagInputProps extends Omit<
  React.InputHTMLAttributes<HTMLInputElement>,
  'value' | 'onChange'
> {
  value: string[];
  onChange: (value: string[]) => void;
  placeholder?: string;
}

export function TagInput({
  value = [],
  onChange,
  placeholder,
  className,
  ...props
}: TagInputProps) {
  const [inputValue, setInputValue] = React.useState('');

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      // Always prevent default to avoid form submission or comma insertion
      e.preventDefault();
      e.stopPropagation();
      const newTag = inputValue.trim();
      if (newTag && !value.includes(newTag)) {
        onChange([...value, newTag]);
        setInputValue('');
      }
    } else if (e.key === 'Backspace' && !inputValue && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };

  const removeTag = (tagToRemove: string) => {
    onChange(value.filter((tag) => tag !== tagToRemove));
  };

  return (
    <div
      className={cn(
        'flex flex-wrap items-center gap-2 rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus-within:ring-2 focus-within:ring-ring focus-within:ring-offset-2',
        className,
      )}
    >
      {value.map((tag) => (
        <Badge
          key={tag}
          variant="secondary"
          className="gap-1 pr-1 font-normal text-sm"
        >
          {tag}
          <button
            type="button"
            onClick={() => removeTag(tag)}
            className="rounded-full hover:bg-muted p-0.5 transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2"
          >
            <X className="h-3 w-3" />
            <span className="sr-only">Remove {tag}</span>
          </button>
        </Badge>
      ))}
      <Input
        {...props}
        value={inputValue}
        onChange={(e) => setInputValue(e.target.value)}
        onKeyDown={handleKeyDown}
        onBlur={(e) => {
          const newTag = inputValue.trim();
          if (newTag && !value.includes(newTag)) {
            onChange([...value, newTag]);
            setInputValue('');
          }
          props.onBlur?.(e);
        }}
        className="flex-1 bg-transparent border-0 p-0 h-6 focus-visible:ring-0 focus-visible:ring-offset-0 min-w-[120px]"
        placeholder={value.length === 0 ? placeholder : ''}
      />
    </div>
  );
}

export { TagInput as TagInputComponent };
