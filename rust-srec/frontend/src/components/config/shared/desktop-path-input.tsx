import * as React from 'react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { FileSearch, FolderOpen, FolderSearch } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { cn } from '@/lib/utils';
import { isLocalDesktopBackend } from '@/desktop/desktop-auth';
import { errorMessage } from '@/lib/api-error';
import { joinPathTemplate, splitPathTemplate } from '@/lib/path-template';
import { openDesktopDirectory, pickDesktopPath } from '@/utils/desktop-dialog';

export interface DesktopPathInputProps extends React.ComponentProps<'input'> {
  pickerMode: 'directory' | 'file';
  onPickValue: (value: string) => void;
  showOpenFolder?: boolean;
}

export const DesktopPathInput = React.forwardRef<
  HTMLInputElement,
  DesktopPathInputProps
>(function DesktopPathInput(
  {
    pickerMode,
    onPickValue,
    showOpenFolder = false,
    className,
    disabled,
    value,
    ...inputProps
  },
  ref,
) {
  const { i18n } = useLingui();
  const [isPicking, setIsPicking] = React.useState(false);
  const [isOpening, setIsOpening] = React.useState(false);

  if (!isLocalDesktopBackend()) {
    return (
      <Input
        ref={ref}
        className={className}
        disabled={disabled}
        value={value}
        {...inputProps}
      />
    );
  }

  const currentValue =
    typeof value === 'string' ? value : value == null ? '' : String(value);
  // Folder templates may contain placeholders: Browse and Open folder act on
  // the folder before the first one, and Browse keeps the placeholder part.
  const { base: existingFolder, suffix: templateSuffix } =
    pickerMode === 'directory'
      ? splitPathTemplate(currentValue.trim())
      : { base: currentValue.trim(), suffix: '' };

  const browseLabel = i18n._(msg`Browse`);
  const browseTitle =
    pickerMode === 'directory'
      ? i18n._(msg`Browse folder`)
      : i18n._(msg`Browse file`);
  const openFolderLabel = i18n._(msg`Open folder`);
  const BrowseIcon = pickerMode === 'directory' ? FolderSearch : FileSearch;

  const handleBrowse = async () => {
    if (isPicking) return;
    setIsPicking(true);
    try {
      const selected = await pickDesktopPath({
        mode: pickerMode,
        defaultPath: existingFolder || undefined,
      });
      if (selected !== null) {
        onPickValue(joinPathTemplate(selected, templateSuffix));
      }
    } catch (error) {
      toast.error(errorMessage(error, i18n._(msg`Failed to open path picker`)));
    } finally {
      setIsPicking(false);
    }
  };

  const handleOpenFolder = async () => {
    if (isOpening || !existingFolder) return;
    setIsOpening(true);
    try {
      await openDesktopDirectory(existingFolder);
    } catch (error) {
      toast.error(errorMessage(error, i18n._(msg`Failed to open folder`)));
    } finally {
      setIsOpening(false);
    }
  };

  return (
    <div className="flex items-center gap-2">
      <Input
        ref={ref}
        className={cn(className, 'flex-1 min-w-0')}
        disabled={disabled}
        value={value}
        {...inputProps}
      />
      <Button
        type="button"
        variant="outline"
        className="h-11 shrink-0 px-3"
        disabled={disabled || isPicking}
        aria-label={browseTitle}
        title={browseTitle}
        onClick={() => {
          void handleBrowse();
        }}
      >
        <BrowseIcon className="h-4 w-4" aria-hidden="true" />
        <span>{browseLabel}</span>
      </Button>
      {showOpenFolder && pickerMode === 'directory' && (
        <Button
          type="button"
          variant="outline"
          size="icon"
          className="h-11 w-11 shrink-0"
          disabled={disabled || isOpening || existingFolder.length === 0}
          aria-label={openFolderLabel}
          title={openFolderLabel}
          onClick={() => {
            void handleOpenFolder();
          }}
        >
          <FolderOpen className="h-4 w-4" aria-hidden="true" />
        </Button>
      )}
    </div>
  );
});

DesktopPathInput.displayName = 'DesktopPathInput';
