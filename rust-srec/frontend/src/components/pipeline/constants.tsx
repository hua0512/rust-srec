import {
    FileVideo,
    Upload,
    Image as ImageIcon,
    Cloud,
    Terminal,
    Copy,
    Scissors,
    Archive,
    Trash,
    Tag,
    Workflow
} from 'lucide-react';
import React from 'react';

export const STEP_ICONS: Record<string, React.ElementType> = {
    remux: FileVideo,
    thumbnail: ImageIcon,
    upload: Upload,
    rclone: Cloud,
    execute: Terminal,
    copy_move: Copy,
    audio_extract: Scissors,
    compression: Archive,
    delete: Trash,
    metadata: Tag,
};

export const STEP_COLORS: Record<string, string> = {
    remux: "from-blue-500/10 to-blue-500/5 text-blue-500 border-blue-500/20",
    thumbnail: "from-purple-500/10 to-purple-500/5 text-purple-500 border-purple-500/20",
    upload: "from-green-500/10 to-green-500/5 text-green-500 border-green-500/20",
    rclone: "from-emerald-500/10 to-emerald-500/5 text-emerald-500 border-emerald-500/20",
    execute: "from-gray-500/10 to-gray-500/5 text-gray-500 border-gray-500/20",
    audio_extract: "from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20",
    audio: "from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20",
    compression: "from-orange-500/10 to-orange-500/5 text-orange-500 border-orange-500/20",
    delete: "from-red-500/10 to-red-500/5 text-red-500 border-red-500/20",
    cleanup: "from-red-500/10 to-red-500/5 text-red-500 border-red-500/20",
    metadata: "from-cyan-500/10 to-cyan-500/5 text-cyan-500 border-cyan-500/20",
    copy_move: "from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20",
    file_ops: "from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20",
    archive: "from-yellow-500/10 to-yellow-500/5 text-yellow-500 border-yellow-500/20",
    custom: "from-slate-500/10 to-slate-500/5 text-slate-500 border-slate-500/20",
};

export function getStepColor(processor: string, category?: string): string {
    if (STEP_COLORS[processor]) return STEP_COLORS[processor];
    if (category && STEP_COLORS[category]) return STEP_COLORS[category];
    return "from-primary/10 to-primary/5 text-primary border-primary/20";
}

export function getStepIcon(processor: string): React.ElementType {
    if (STEP_ICONS[processor]) return STEP_ICONS[processor];
    for (const [key, Icon] of Object.entries(STEP_ICONS)) {
        if (processor.startsWith(key)) return Icon;
    }
    return Workflow;
}
