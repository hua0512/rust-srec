import { useWatch } from "react-hook-form";
import {
    FormControl,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
    FormDescription,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { type RcloneConfigSchema } from "../processor-schemas";
import { z } from "zod";
import { ListInput } from "@/components/ui/list-input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import {
    Cloud,
    Settings2,
    Terminal,
    ArrowRightLeft,
    Copy,
    Move,
    RefreshCw
} from "lucide-react";
import { ProcessorConfigFormProps } from "./common-props";

type RcloneConfig = z.infer<typeof RcloneConfigSchema>;

export function RcloneConfigForm({ control, pathPrefix }: ProcessorConfigFormProps<RcloneConfig>) {
    const prefix = pathPrefix ? `${pathPrefix}.` : "";
    const operation = useWatch({
        control,
        name: `${prefix}operation` as any,
    });

    const getOperationIcon = () => {
        switch (operation) {
            case "move":
                return <Move className="h-4 w-4 text-orange-400" />;
            case "sync":
                return <RefreshCw className="h-4 w-4 text-blue-400" />;
            case "copy":
            default:
                return <Copy className="h-4 w-4 text-green-400" />;
        }
    };

    return (
        <Tabs defaultValue="general" className="w-full">
            <TabsList className="grid w-full grid-cols-2 mb-4 bg-muted/20 p-1">
                <TabsTrigger
                    value="general"
                    className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
                >
                    General
                </TabsTrigger>
                <TabsTrigger
                    value="advanced"
                    className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
                >
                    Advanced
                </TabsTrigger>
            </TabsList>

            <TabsContent value="general" className="space-y-4">
                {/* Operation Selection */}
                <Card className="border-border/50 bg-muted/10 shadow-sm">
                    <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
                        <div className="flex items-center gap-2">
                            <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                                <ArrowRightLeft className="h-4 w-4 text-primary" />
                            </div>
                            <CardTitle className="text-sm font-medium">Operation Mode</CardTitle>
                        </div>
                    </CardHeader>
                    <CardContent className="grid gap-4 pt-4">
                        <FormField
                            control={control}
                            name={`${prefix}operation` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel>Operation</FormLabel>
                                    <Select
                                        onValueChange={field.onChange}
                                        defaultValue={field.value}
                                    >
                                        <FormControl>
                                            <SelectTrigger className="h-11 bg-background/50">
                                                <div className="flex items-center gap-2">
                                                    {getOperationIcon()}
                                                    <SelectValue placeholder="Select operation" />
                                                </div>
                                            </SelectTrigger>
                                        </FormControl>
                                        <SelectContent>
                                            <SelectItem value="copy">
                                                <div className="flex items-center gap-2">
                                                    <Copy className="h-4 w-4 text-green-400" />
                                                    <span>Copy</span>
                                                    <span className="ml-2 text-xs text-muted-foreground/50">
                                                        (Preserve source)
                                                    </span>
                                                </div>
                                            </SelectItem>
                                            <SelectItem value="move">
                                                <div className="flex items-center gap-2">
                                                    <Move className="h-4 w-4 text-orange-400" />
                                                    <span>Move</span>
                                                    <span className="ml-2 text-xs text-muted-foreground/50">
                                                        (Delete source)
                                                    </span>
                                                </div>
                                            </SelectItem>
                                            <SelectItem value="sync">
                                                <div className="flex items-center gap-2">
                                                    <RefreshCw className="h-4 w-4 text-blue-400" />
                                                    <span>Sync</span>
                                                    <span className="ml-2 text-xs text-muted-foreground/50">
                                                        (Mirror source)
                                                    </span>
                                                </div>
                                            </SelectItem>
                                        </SelectContent>
                                    </Select>
                                    <FormDescription>
                                        Choose how files are transferred to the remote.
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </CardContent>
                </Card>

                {/* Target Configuration */}
                <Card className="border-border/50 bg-muted/10 shadow-sm">
                    <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
                        <div className="flex items-center gap-2">
                            <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                                <Cloud className="h-4 w-4 text-primary" />
                            </div>
                            <CardTitle className="text-sm font-medium">Target Configuration</CardTitle>
                        </div>
                    </CardHeader>
                    <CardContent className="grid gap-4 pt-4">
                        <FormField
                            control={control}
                            name={`${prefix}destination_root` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel>Destination Root</FormLabel>
                                    <FormControl>
                                        <Input
                                            placeholder="e.g. gdrive:/videos"
                                            {...field}
                                            className="h-11 bg-background/50 font-mono text-sm"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        Base path for remote storage.
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <div className="grid grid-cols-2 gap-4">
                            <FormField
                                control={control}
                                name={`${prefix}config_path` as any}
                                render={({ field }) => (
                                    <FormItem>
                                        <FormLabel>Config Path (Optional)</FormLabel>
                                        <FormControl>
                                            <Input
                                                placeholder="/path/to/rclone.conf"
                                                {...field}
                                                className="bg-background/50"
                                            />
                                        </FormControl>
                                        <FormMessage />
                                    </FormItem>
                                )}
                            />
                            <FormField
                                control={control}
                                name={`${prefix}rclone_path` as any}
                                render={({ field }) => (
                                    <FormItem>
                                        <FormLabel>Rclone Executable</FormLabel>
                                        <FormControl>
                                            <Input {...field} className="bg-background/50" />
                                        </FormControl>
                                        <FormMessage />
                                    </FormItem>
                                )}
                            />
                        </div>
                    </CardContent>
                </Card>
            </TabsContent>

            <TabsContent value="advanced" className="space-y-4">
                {/* Retry Policy */}
                <Card className="border-border/50 bg-muted/10 shadow-sm">
                    <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
                        <div className="flex items-center gap-2">
                            <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                                <Settings2 className="h-4 w-4 text-primary" />
                            </div>
                            <CardTitle className="text-sm font-medium">Retry Policy</CardTitle>
                        </div>
                    </CardHeader>
                    <CardContent className="grid gap-4 pt-4">
                        <FormField
                            control={control}
                            name={`${prefix}max_retries` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel>Max Retries</FormLabel>
                                    <FormControl>
                                        <Input
                                            type="number"
                                            {...field}
                                            onChange={(e) => field.onChange(parseInt(e.target.value))}
                                            className="bg-background/50"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        Number of attempts before failing the upload.
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </CardContent>
                </Card>

                {/* Arguments */}
                <Card className="border-border/50 bg-muted/10 shadow-sm">
                    <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
                        <div className="flex items-center gap-2">
                            <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                                <Terminal className="h-4 w-4 text-primary" />
                            </div>
                            <CardTitle className="text-sm font-medium">Extra Arguments</CardTitle>
                        </div>
                    </CardHeader>
                    <CardContent className="pt-4">
                        <FormField
                            control={control}
                            name={`${prefix}args` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormControl>
                                        <ListInput
                                            value={field.value || []}
                                            onChange={field.onChange}
                                            placeholder="Add rclone argument"
                                        />
                                    </FormControl>
                                    <FormDescription>
                                        Double click to edit items.
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </CardContent>
                </Card>
            </TabsContent>
        </Tabs>
    );
}
