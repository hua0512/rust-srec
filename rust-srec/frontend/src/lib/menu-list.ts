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
import { msg } from '@lingui/core/macro';
import { type I18n } from '@lingui/core';

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

export function getMenuList(_pathname: string, i18n: I18n): Group[] {
  return [
    {
      groupLabel: '',
      menus: [
        {
          href: '/dashboard',
          label: i18n._(msg`Dashboard`),
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
          label: i18n._(msg`Streamers`),
          icon: Users,
          submenus: [],
        },
        {
          href: '/sessions',
          label: i18n._(msg`Sessions`),
          icon: Film,
          submenus: [],
        },
        {
          href: '/pipeline',
          label: i18n._(msg`Pipeline`),
          icon: Workflow,
          submenus: [
            {
              href: '/pipeline/presets',
              label: i18n._(msg`Presets`),
              icon: Settings2,
            },
            {
              href: '/pipeline/workflows',
              label: i18n._(msg`Workflows`),
              icon: GitBranch,
            },
            {
              href: '/pipeline/jobs',
              label: i18n._(msg`Jobs`),
              icon: ListTodo,
            },
            {
              href: '/pipeline/outputs',
              label: i18n._(msg`Outputs`),
              icon: FileVideo,
            },
          ],
        },
        {
          href: '/player',
          label: i18n._(msg`Player`),
          icon: Play,
          submenus: [],
        },
      ],
    },

    {
      groupLabel: i18n._(msg`Settings`),
      menus: [
        {
          href: '/system/health',
          label: i18n._(msg`System Health`),
          icon: Activity,
        },
        // {
        //     href: "/users", // TODO: /users
        //     label: "Users",
        //     icon: Users
        // },
        {
          href: '/config',
          label: i18n._(msg`Configuration`),
          icon: Settings,
        },
        {
          href: '/notifications',
          label: i18n._(msg`Notifications`),
          icon: Bell,
        },
      ],
    },
  ];
}
