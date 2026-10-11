import { Card, CardContent } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Trans } from '@lingui/react/macro';
import { Cookie, Tv, Edit, Clock, Download } from 'lucide-react';
import { PlatformConfigSchema } from '@/api/schemas';
import { z } from 'zod';
import {
  getPlatformIcon,
  getPlatformColor,
} from '@/components/pipeline/constants';
import { cn } from '@/lib/utils';
import { formatPlatformName } from '@/lib/format';
import { CardField } from '@/components/shared/card-field';
import {
  CardDecoration,
  CardHeaderRow,
  CardIconTile,
  CardMenuButton,
  CardMetaFooter,
} from '@/components/shared/decorated-card';

interface PlatformCardProps {
  platform: z.infer<typeof PlatformConfigSchema>;
  onEdit: () => void;
}

function formatDelay(ms: number | null | undefined) {
  return ms ? (
    `${(ms / 1000).toFixed(0)}s`
  ) : (
    <span className="opacity-50">
      <Trans>default</Trans>
    </span>
  );
}

export function PlatformCard({ platform, onEdit }: PlatformCardProps) {
  const Icon = getPlatformIcon(platform.name);
  const colorClass = getPlatformColor(platform.name);

  return (
    <Card className="h-full flex flex-col border-border/50 bg-card/50 shadow-sm hover:shadow-md transition-all duration-300 hover:border-primary/20 group relative overflow-hidden">
      <CardDecoration line={false} />

      <CardHeaderRow
        className="px-4 pt-4"
        icon={
          <CardIconTile
            icon={Icon}
            className={cn(
              'rounded-xl group-hover:scale-105 group-hover:rotate-0',
              colorClass,
            )}
          />
        }
        title={formatPlatformName(platform.name)}
        titleClassName="font-semibold"
        subtitle={
          <>
            {platform.record_danmu && (
              <div className="flex items-center gap-1 text-green-600 dark:text-green-400 bg-green-500/10 px-1.5 py-0.5 rounded-full border border-green-500/10">
                <Tv className="w-3 h-3" />
                <span>Danmu</span>
              </div>
            )}
            {/* Only a selection that names accounts; inherit/none sign in anonymously. */}
            {(platform.credential_selection?.mode === 'fixed' ||
              platform.credential_selection?.mode === 'pool') && (
              <div className="flex items-center gap-1 text-orange-600 dark:text-orange-400 bg-orange-500/10 px-1.5 py-0.5 rounded-full border border-orange-500/10">
                <Cookie className="w-3 h-3" />
                <span>Accounts</span>
              </div>
            )}
          </>
        }
        actions={
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <CardMenuButton className="text-muted-foreground/70" />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuItem onClick={onEdit}>
                <Edit className="mr-2 h-4 w-4" /> <Trans>Edit</Trans>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        }
      />

      <CardContent className="relative pb-4 flex-1 z-10 pt-4 px-4">
        <div className="grid grid-cols-2 gap-3">
          <CardField
            size="md"
            icon={Clock}
            label={<Trans>Fetch</Trans>}
            value={formatDelay(platform.fetch_delay_ms)}
            mono
          />
          <CardField
            size="md"
            icon={Download}
            label={<Trans>Wait</Trans>}
            value={formatDelay(platform.download_delay_ms)}
            mono
          />
          {platform.output_folder && (
            <CardField
              size="md"
              className="col-span-2"
              label={<Trans>Output</Trans>}
              value={platform.output_folder}
              mono
            />
          )}
          {platform.download_engine && (
            <CardField
              size="md"
              className="col-span-2"
              label={<Trans>Engine</Trans>}
              value={platform.download_engine}
              mono
            />
          )}
        </div>
      </CardContent>

      <CardMetaFooter className="border-border/30 bg-muted/20">
        <span className="font-mono opacity-50">
          #{platform.name.toLowerCase()}
        </span>
      </CardMetaFooter>
    </Card>
  );
}
