import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "./ui/sidebar"
import { Link } from "@tanstack/react-router"
import { Users, Film, Activity, Settings, LogOut, LayoutGrid } from "lucide-react"
import { useAuth } from "../hooks/useAuth"

const items = [
  {
    title: "Dashboard",
    url: "/dashboard",
    icon: LayoutGrid,
  },
  {
    title: "Streamers",
    url: "/streamers",
    icon: Users,
  },
  {
    title: "Sessions",
    url: "/sessions",
    icon: Film,
  },
  {
    title: "Pipeline",
    url: "/pipeline/jobs",
    icon: Activity,
  },
  {
    title: "Configuration",
    url: "/config/global",
    icon: Settings,
  },
]

export function AppSidebar() {
  const { logout } = useAuth()

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <div className="flex items-center gap-2 px-2 py-1 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0 group-data-[collapsible=icon]:gap-0">
          <div className="flex h-10 w-10 items-center justify-center">
            <img src="/stream-rec.svg" alt="stream-rec" className="size-8 dark:brightness-0 dark:invert" />
          </div>
          <span className="text-lg font-bold transition-all duration-200 group-data-[collapsible=icon]:w-0 group-data-[collapsible=icon]:opacity-0 overflow-hidden whitespace-nowrap">Stream-rec</span>
        </div>
      </SidebarHeader>
      <SidebarContent className="pt-8">
        <SidebarGroup>
          <SidebarGroupLabel className="px-6">Application</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {items.map((item) => (
                <SidebarMenuItem key={item.title}>
                  <SidebarMenuButton asChild tooltip={item.title}>
                    <Link to={item.url} activeProps={{ className: "bg-sidebar-accent text-sidebar-accent-foreground" }}>
                      <item.icon />
                      <span>{item.title}</span>
                    </Link>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton onClick={logout}>
              <LogOut />
              <span>Logout</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  )
}
