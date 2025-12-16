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
  Settings2,
  Activity,
  Play,
  Bell,
} from 'lucide-react';
import { t } from '@lingui/core/macro';

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
      groupLabel: '',
      menus: [
        {
          href: '/dashboard',
          label: t`Dashboard`,
          icon: LayoutGrid,
          submenus: [],
        },
      ],
    },

    {
      groupLabel: '',
      menus: [
        {
          href: '/streamers',
          label: t`Streamers`,
          icon: Users,
          submenus: [],
        },
        {
          href: '/sessions',
          label: t`Sessions`,
          icon: Film,
          submenus: [],
        },
        {
          href: '/pipeline',
          label: t`Pipeline`,
          icon: Workflow,
          submenus: [
            {
              href: '/pipeline/presets',
              label: t`Presets`,
              icon: Settings2,
            },
            {
              href: '/pipeline/workflows',
              label: t`Workflows`,
              icon: GitBranch,
            },
            {
              href: '/pipeline/jobs',
              label: t`Jobs`,
              icon: ListTodo,
            },
            {
              href: '/pipeline/outputs',
              label: t`Outputs`,
              icon: FileVideo,
            },
          ],
        },
        {
          href: '/player',
          label: t`Player`,
          icon: Play,
          submenus: [],
        },
      ],
    },

    {
      groupLabel: t`Settings`,
      menus: [
        {
          href: '/system/health',
          label: t`System Health`,
          icon: Activity,
        },
        // {
        //     href: "/users", // TODO: /users
        //     label: "Users",
        //     icon: Users
        // },
        {
          href: '/config',
          label: t`Configuration`,
          icon: Settings,
        },
        {
          href: '/notifications',
          label: t`Notifications`,
          icon: Bell,
        },
      ],
    },
  ];
}
