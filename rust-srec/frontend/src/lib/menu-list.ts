import {
    Users,
    Settings,
    LayoutGrid,
    LucideIcon,
    Film,
    Workflow,
    ListTodo,
    FileVideo,
    GitBranch,
    Settings2
} from "lucide-react";

type Submenu = {
    href: string;
    label: string;
    active?: boolean;
    icon?: LucideIcon;
};

type Menu = {
    href: string;
    label: string;
    active?: boolean;
    icon: LucideIcon;
    submenus?: Submenu[];
};

type Group = {
    groupLabel: string;
    menus: Menu[];
};

export function getMenuList(_pathname: string): Group[] {
    return [
        {
            groupLabel: "",
            menus: [
                {
                    href: "/dashboard",
                    label: "Dashboard",
                    icon: LayoutGrid,
                    submenus: []
                }
            ]
        },


        {
            groupLabel: "",
            menus: [
                {
                    href: "/streamers",
                    label: "Streamers",
                    icon: Users,
                    submenus: []
                },
                {
                    href: "/sessions",
                    label: "Sessions",
                    icon: Film,
                    submenus: []
                },
                {
                    href: "/pipeline",
                    label: "Pipeline",
                    icon: Workflow,
                    submenus: [
                        {
                            href: "/pipeline/presets",
                            label: "Presets",
                            icon: Settings2
                        },
                        {
                            href: "/pipeline/workflows",
                            label: "Workflows",
                            icon: GitBranch
                        },
                        {
                            href: "/pipeline/jobs",
                            label: "Jobs",
                            icon: ListTodo
                        },
                        {
                            href: "/pipeline/outputs",
                            label: "Outputs",
                            icon: FileVideo
                        }
                    ]
                },
            ]
        },

        {
            groupLabel: "Settings",
            menus: [
                // {
                //     href: "/users", // TODO: /users
                //     label: "Users",
                //     icon: Users
                // },
                {
                    href: "/config",
                    label: "Configuration",
                    icon: Settings
                }
            ]
        }
    ];
}