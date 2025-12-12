import { Control } from "react-hook-form";
import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
} from "@/components/ui/card";
import {
    FormControl,
    FormDescription,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { FlagFormField } from "@/components/ui/flag-form-field";
import { FolderOutput } from "lucide-react";
import { Trans } from "@lingui/react/macro";

interface FileConfigCardProps {
    control: Control<any>;
}

export function FileConfigCard({ control }: FileConfigCardProps) {
    return (
        <Card className="h-full hover:shadow-md transition-all duration-300 border-muted/60">
            <CardHeader>
                <CardTitle className="flex items-center gap-3 text-xl">
                    <div className="p-2.5 bg-blue-500/10 text-blue-500 rounded-lg">
                        <FolderOutput className="w-5 h-5" />
                    </div>
                    <Trans>File Configuration</Trans>
                </CardTitle>
                <CardDescription className="pl-[3.25rem]">
                    <Trans>Output paths, templates, and formats.</Trans>
                </CardDescription>
            </CardHeader>
            <CardContent className="space-y-6">
                <FlagFormField
                    control={control}
                    fieldName="record_danmu"
                    title={<Trans>Record Danmu</Trans>}
                    description={
                        <Trans>
                            Enable recording of danmu/chat messages along with the video.
                        </Trans>
                    }
                />

                <div className="space-y-6">
                    <FormField
                        control={control}
                        name="output_folder"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Output Folder</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input placeholder="/path/to/downloads" {...field} />
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />

                    <FormField
                        control={control}
                        name="output_filename_template"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Filename Template</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input
                                        placeholder="{streamer}-{title}-%Y%m%d-%H%M%S"
                                        {...field}
                                    />
                                </FormControl>
                                <FormDescription className="text-xs">
                                    <Trans>
                                        Vars: '&#123;streamer&#125;', '&#123;title&#125;' | Time:
                                        %Y, %m, %d, %H, %M, %S
                                    </Trans>
                                </FormDescription>
                                <FormMessage />
                            </FormItem>
                        )}
                    />

                    <FormField
                        control={control}
                        name="output_file_format"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel>
                                    <Trans>Format</Trans>
                                </FormLabel>
                                <FormControl>
                                    <Input placeholder="mp4" {...field} />
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </div>
            </CardContent>
        </Card>
    );
}
