import React from 'react';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Textarea } from '@/components/ui/textarea';
import { Trans } from '@lingui/react/macro';
import type { ImportedTheme } from '@/types/theme-customizer';

interface ImportModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onImport: (theme: ImportedTheme) => void;
}

/** Collects the `--name: value;` declarations inside the first block matching `block`. */
function readVariables(css: string, block: RegExp): Record<string, string> {
  const variables: Record<string, string> = {};
  const content = css.match(block)?.[1];
  if (!content) return variables;
  for (const [, variable, value] of content.matchAll(
    /--([^:]+):\s*([^;]+);/g,
  )) {
    variables[variable.trim()] = value.trim();
  }
  return variables;
}

export function ImportModal({
  open,
  onOpenChange,
  onImport,
}: ImportModalProps) {
  const [importText, setImportText] = React.useState('');

  const processImport = () => {
    const cssText = importText.replace(/\/\*[\s\S]*?\*\//g, ''); // Remove comments
    const theme: ImportedTheme = {
      light: readVariables(cssText, /:root\s*\{([^}]+)\}/),
      dark: readVariables(cssText, /\.dark\s*\{([^}]+)\}/),
    };

    try {
      onImport(theme);
    } catch (error) {
      // Persisting the theme can fail, e.g. when storage is full; keep the
      // dialog open with the text so nothing is lost.
      console.error('Error importing theme:', error);
      return;
    }

    onOpenChange(false);
    setImportText('');
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange} modal={true}>
      <DialogContent className="max-w-4xl w-[90vw]">
        <DialogHeader>
          <DialogTitle>
            <Trans>Import Custom CSS</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Paste your CSS theme below. Include both <code>:root</code> (light
              mode) and <code>.dark</code> (dark mode) sections with CSS
              variables like <code>--primary</code>, <code>--background</code>,
              etc. The theme will automatically switch between light and dark
              modes.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <Textarea
              className="shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring max-h-[400px] min-h-[300px] font-mono text-sm text-foreground overflow-y-auto resize-none"
              placeholder={`:root {
  --background: 0 0% 100%;
  --foreground: oklch(0.52 0.13 144.17);
  --primary: #3e2723;
  /* And more */
}
.dark {
  --background: 222.2 84% 4.9%;
  --foreground: hsl(37.50 36.36% 95.69%);
  --primary: rgb(46, 125, 50);
  /* And more */
}`}
              value={importText}
              onChange={(e) => setImportText(e.target.value)}
            />
          </div>
          <div className="flex gap-2 justify-end">
            <Button
              variant="outline"
              onClick={() => onOpenChange(false)}
              className="cursor-pointer"
            >
              <Trans>Cancel</Trans>
            </Button>
            <Button
              onClick={processImport}
              disabled={!importText.trim()}
              className="cursor-pointer"
            >
              <Trans>Import Theme</Trans>
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
