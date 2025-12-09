import { SidebarTrigger } from "./ui/sidebar"
import { Separator } from "./ui/separator"
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "./ui/breadcrumb"
import { useLocation } from "@tanstack/react-router"
import { ThemeToggle } from "./theme-toggle"
import { LanguageSwitcher } from "./language-switcher"
import { Input } from "./ui/input"
import { Search } from "lucide-react"

export function TopBar() {
  const location = useLocation()
  const pathSegments = location.pathname.split("/").filter(Boolean)

  return (
    <header className="flex h-16 shrink-0 items-center gap-2 border-b px-4">
      <div className="flex items-center gap-2 px-4">
        <SidebarTrigger className="-ml-1" />
        <Separator orientation="vertical" className="mr-2 h-4" />
        <Breadcrumb>
          <BreadcrumbList>
            <BreadcrumbItem>
                <BreadcrumbLink href="/dashboard">Home</BreadcrumbLink>
            </BreadcrumbItem>
            {pathSegments.map((segment, index) => {
                const isLast = index === pathSegments.length - 1
                const href = `/${pathSegments.slice(0, index + 1).join("/")}`
                return (
                    <div key={href} className="flex items-center gap-2">
                        <BreadcrumbSeparator />
                        <BreadcrumbItem>
                            {isLast ? (
                                <BreadcrumbPage className="capitalize">{segment}</BreadcrumbPage>
                            ) : (
                                <BreadcrumbLink href={href} className="capitalize">{segment}</BreadcrumbLink>
                            )}
                        </BreadcrumbItem>
                    </div>
                )
            })}
          </BreadcrumbList>
        </Breadcrumb>
      </div>
      <div className="ml-auto flex items-center gap-4">
        <div className="relative w-64 hidden md:block">
            <Search className="absolute left-2 top-2.5 h-4 w-4 text-muted-foreground" />
            <Input placeholder="Search..." className="pl-8" />
        </div>
        <LanguageSwitcher />
        <ThemeToggle />
      </div>
    </header>
  )
}
