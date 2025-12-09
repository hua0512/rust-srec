import {
    Users,
    Settings,
    LayoutGrid,
    LucideIcon,
    Film
} from "lucide-react";

type Submenu = {
    href: string;
    label: string;
    active?: boolean;
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

export function getMenuList(pathname: string): Group[] {
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
                }
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
                    href: "/config/global",
                    label: "Global",
                    icon: Settings
                }
            ]
        }
    ];
}