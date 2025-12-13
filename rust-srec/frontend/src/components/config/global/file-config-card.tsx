import { Control } from "react-hook-form";
import { SettingsCard } from "../settings-card";
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
        <SettingsCard
            title={<Trans>File Configuration</Trans>}
            description={<Trans>Output paths, templates, and formats.</Trans>}
            icon={FolderOutput}
            iconColor="text-blue-500"
            iconBgColor="bg-blue-500/10"
        >
            <div className="space-y-6">
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
            </div>
        </SettingsCard>
    );
}
