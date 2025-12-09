import { Languages } from "lucide-react"
import { Button } from "./ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu"
import { useLingui } from "@lingui/react"

export function LanguageSwitcher() {
  const { i18n } = useLingui()

  const changeLocale = (locale: string) => {
    i18n.activate(locale)
    localStorage.setItem("locale", locale)
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="icon">
          <Languages className="h-[1.2rem] w-[1.2rem]" />
          <span className="sr-only">Switch language</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => changeLocale("en")}>
          English
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => changeLocale("zh-CN")}>
          中文 (简体)
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
