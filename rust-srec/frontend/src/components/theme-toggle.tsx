import { flushSync } from "react-dom"
import { Moon, Sun } from "lucide-react"
import { Button } from "./ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu"
import { useTheme } from "./theme-provider"

export function ThemeToggle() {
  const { theme: currentTheme, setTheme } = useTheme()

  const handleThemeChange = (theme: "light" | "dark" | "system", e: React.MouseEvent) => {
    if (currentTheme === theme) {
      return
    }

    // We need the coordinates *before* the transition starts.
    const x = e.clientX
    const y = e.clientY

    if (
      !document.startViewTransition ||
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      setTheme(theme)
      return
    }

    const endRadius = Math.hypot(
      Math.max(x, innerWidth - x),
      Math.max(y, innerHeight - y)
    )

    const transition = document.startViewTransition(() => {
      flushSync(() => {
        setTheme(theme)
      })
    })

    transition.ready.then(() => {
      const clipPath = [
        `circle(0px at ${x}px ${y}px)`,
        `circle(${endRadius}px at ${x}px ${y}px)`,
      ]

      const isDark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches)

      document.documentElement.animate(
        {
          clipPath: isDark ? clipPath : [...clipPath].reverse(),
        },
        {
          duration: 400,
          easing: "ease-in",
          pseudoElement: isDark ? "::view-transition-new(root)" : "::view-transition-old(root)",
          fill: "forwards",
        }
      )
    })
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="icon">
          <Sun className="h-[1.2rem] w-[1.2rem] rotate-0 scale-100 transition-all dark:-rotate-90 dark:scale-0" />
          <Moon className="absolute h-[1.2rem] w-[1.2rem] rotate-90 scale-0 transition-all dark:rotate-0 dark:scale-100" />
          <span className="sr-only">Toggle theme</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={(e) => handleThemeChange("light", e)}>
          Light
        </DropdownMenuItem>
        <DropdownMenuItem onClick={(e) => handleThemeChange("dark", e)}>
          Dark
        </DropdownMenuItem>
        <DropdownMenuItem onClick={(e) => handleThemeChange("system", e)}>
          System
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
