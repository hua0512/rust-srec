import { UseFormReturn } from 'react-hook-form';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Trash2 } from 'lucide-react';
import { FfmpegForm } from '../../engines/forms/ffmpeg-form';
import { StreamlinkForm } from '../../engines/forms/streamlink-form';
import { MesioForm } from '../../engines/forms/mesio-form';

interface EngineOverrideCardProps {
    engineId: string;
    engineName: string;
    engineType: string;
    form: UseFormReturn<any>;
    onRemove: () => void;
}

export function EngineOverrideCard({ engineId, engineName, engineType, form, onRemove }: EngineOverrideCardProps) {
    // Determine which form to render based on engine type
    const renderEngineForm = () => {
        const basePath = `engines_override.${engineId}`;
        switch (engineType) {
            case 'FFMPEG':
                return <FfmpegForm control={form.control} basePath={basePath} />;
            case 'STREAMLINK':
                return <StreamlinkForm control={form.control} basePath={basePath} />;
            case 'MESIO':
                return <MesioForm control={form.control} basePath={basePath} />;
            default:
                return <div className="text-muted-foreground p-4">Unknown engine type: {engineType}</div>;
        }
    };

    return (
        <Card className="relative overflow-hidden transition-all hover:border-primary/50">
            <CardHeader className="pb-3 bg-muted/20 border-b flex flex-row items-center justify-between space-y-0">
                <div className="flex items-center gap-3">
                    <CardTitle className="text-base font-semibold">{engineName}</CardTitle>
                    <Badge variant="outline" className="font-mono text-xs">
                        {engineType}
                    </Badge>
                </div>
                <Button
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 text-muted-foreground hover:text-destructive transition-colors"
                    onClick={onRemove}
                >
                    <Trash2 className="w-4 h-4" />
                </Button>
            </CardHeader>
            <CardContent className="pt-6">
                {renderEngineForm()}
            </CardContent>
        </Card>
    );
}
