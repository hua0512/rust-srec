import { JobPresetSchema } from '@/api/schemas';
import { CardContent } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog';
import { Edit, Trash, CopyPlus } from 'lucide-react';
import { z } from 'zod';
import { CardField } from '@/components/shared/card-field';
import {
  CardHeaderRow,
  CardIconTile,
  CardMenuButton,
  CardMetaFooter,
  DecoratedCard,
} from '@/components/shared/decorated-card';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import {
  getStepBadgeColor,
  getStepIcon,
} from '@/components/pipeline/constants';
import {
  DEFAULT_JOB_PRESET_DESCRIPTIONS,
  DEFAULT_JOB_PRESET_NAMES,
  getCategoryName,
} from './default-presets-i18n';

// Object values would otherwise render as "[object Object]".
function formatConfigValue(value: unknown): string {
  return value !== null && typeof value === 'object'
    ? JSON.stringify(value)
    : String(value);
}

interface PresetCardProps {
  preset: z.infer<typeof JobPresetSchema>;
  onEdit: (preset: z.infer<typeof JobPresetSchema>) => void;
  onDelete: (id: string) => void;
  onClone?: (preset: z.infer<typeof JobPresetSchema>) => void;
}

export function PresetCard({
  preset,
  onEdit,
  onDelete,
  onClone,
}: PresetCardProps) {
  const { i18n } = useLingui();
  const Icon = getStepIcon(preset.processor);
  const colorClass = getStepBadgeColor(
    preset.processor,
    preset.category ?? undefined,
  );
  const categoryLabel = preset.category
    ? getCategoryName(preset.category, i18n)
    : null;

  let configObj: any = {};
  try {
    configObj =
      typeof preset.config === 'string'
        ? JSON.parse(preset.config)
        : preset.config;
  } catch {}

  const description = DEFAULT_JOB_PRESET_DESCRIPTIONS[preset.id]
    ? i18n._(DEFAULT_JOB_PRESET_DESCRIPTIONS[preset.id])
    : preset.description;

  const configKeys = Object.keys(configObj).filter(
    (k) => k !== 'overwrite' && k !== 'create_dirs',
  );

  return (
    <DecoratedCard>
      <CardHeaderRow
        icon={<CardIconTile icon={Icon} className={colorClass} />}
        title={
          DEFAULT_JOB_PRESET_NAMES[preset.id]
            ? i18n._(DEFAULT_JOB_PRESET_NAMES[preset.id])
            : preset.name
        }
        subtitle={categoryLabel}
        actions={
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <CardMenuButton />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onClick={() => onEdit(preset)}>
                <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
              </DropdownMenuItem>
              {onClone && (
                <DropdownMenuItem onClick={() => onClone(preset)}>
                  <CopyPlus className="mr-2 h-4 w-4" /> <Trans>Clone</Trans>
                </DropdownMenuItem>
              )}
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <DropdownMenuItem
                    className="text-destructive focus:text-destructive"
                    onSelect={(e) => e.preventDefault()}
                  >
                    <Trash className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                  </DropdownMenuItem>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>
                      <Trans>Delete Preset?</Trans>
                    </AlertDialogTitle>
                    <AlertDialogDescription>
                      <Trans>
                        This will permanently delete the preset "{preset.name}".
                        Pipelines using this preset may fail if not updated.
                      </Trans>
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>
                      <Trans>Cancel</Trans>
                    </AlertDialogCancel>
                    <AlertDialogAction
                      onClick={() => onDelete(preset.id)}
                      className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
                    >
                      <Trans>Delete</Trans>
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            </DropdownMenuContent>
          </DropdownMenu>
        }
      />
      <CardContent className="relative pb-4 flex-1 z-10">
        {/* Description or Config Summary */}
        <div className="text-xs text-muted-foreground/70 mb-4 line-clamp-2 leading-relaxed min-h-[2.5em]">
          {description || (
            <span className="italic opacity-40 text-xs">
              <Trans>No description.</Trans>
            </span>
          )}
        </div>
        <div className="text-sm text-muted-foreground">
          {configKeys.length > 0 ? (
            <div className="grid grid-cols-2 gap-2">
              {configKeys.slice(0, 4).map((key) => (
                <CardField
                  key={key}
                  title={`${key}: ${formatConfigValue(configObj[key])}`}
                  label={key}
                  value={formatConfigValue(configObj[key])}
                />
              ))}
            </div>
          ) : (
            <span className="italic opacity-40 text-xs">
              <Trans>No configuration.</Trans>
            </span>
          )}
        </div>
      </CardContent>
      <CardMetaFooter>
        <span className="font-mono opacity-50">#{preset.processor}</span>
      </CardMetaFooter>
    </DecoratedCard>
  );
}
