import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Badge } from '@/components/ui/badge';
import { Globe } from 'lucide-react';

const SUPPORTED_PLATFORMS = [
  'acfun',
  'bilibili',
  'douyin',
  'douyu',
  'huya',
  'pandatv',
  'picarto',
  'redbook',
  'tiktok',
  'twitcasting',
  'twitch',
  'weibo',
].sort();

export function SupportedPlatforms() {
  return (
    <Card className="border-muted/60 shadow-sm bg-muted/20">
      <CardHeader className="pb-3">
        <CardTitle className="flex items-center gap-2 text-lg font-medium">
          <Globe className="w-4 h-4 text-muted-foreground" />
          <Trans>Supported Platforms</Trans>
        </CardTitle>
      </CardHeader>
      <CardContent>
        <div className="flex flex-wrap gap-2">
          {SUPPORTED_PLATFORMS.map((platform) => (
            <Badge
              key={platform}
              variant="secondary"
              className="font-mono text-xs cursor-default hover:bg-secondary/80 transition-colors"
            >
              {platform}
            </Badge>
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
