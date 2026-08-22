import { Input } from '@/components/ui/input';
import { Search } from 'lucide-react';
import { useEffect, useState, memo } from 'react';

interface SearchInputProps {
  defaultValue?: string;
  onSearch: (value: string) => void;
  placeholder?: string;
  className?: string;
}

export const SearchInput = memo(
  ({
    defaultValue = '',
    onSearch,
    placeholder,
    className,
  }: SearchInputProps) => {
    const [value, setValue] = useState(defaultValue);
    const [isComposing, setIsComposing] = useState(false);

    // Sync internal value with defaultValue (e.g., when clearing filters)
    useEffect(() => {
      if (!isComposing && value !== defaultValue) {
        setValue(defaultValue);
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [defaultValue]);

    // Handle debounced search
    useEffect(() => {
      if (isComposing) return;

      const timer = setTimeout(() => {
        if (value !== defaultValue) {
          onSearch(value);
        }
      }, 400);

      return () => clearTimeout(timer);
    }, [value, isComposing, onSearch, defaultValue]);

    return (
      <div className={`relative w-full ${className}`}>
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
        <Input
          placeholder={placeholder}
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onCompositionStart={() => setIsComposing(true)}
          onCompositionEnd={(e) => {
            setIsComposing(false);
            // Set value explicitly on composition end to ensure sync
            const committedValue = (e.target as HTMLInputElement).value;
            setValue(committedValue);
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !isComposing) {
              onSearch(value);
            }
          }}
          className="pl-9 h-9 bg-muted/40 border-border/50 focus:bg-background transition-colors"
        />
      </div>
    );
  },
);
