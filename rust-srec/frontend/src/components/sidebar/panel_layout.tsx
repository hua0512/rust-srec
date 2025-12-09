import { useStore } from "@/hooks/use-store";
import { cn } from "@/lib/utils";
import { Footer } from "./footer";
import { useSidebar } from "@/store/sidebar";
import { Sidebar } from "./sidebar";
import { Navbar } from "./navbar";

export default function DashboardPanelLayout({
    children
}: {
    children: React.ReactNode;
}) {
    const sidebar = useStore(useSidebar, (x) => x);
    if (!sidebar) return null;
    const { getOpenState, settings } = sidebar;
    return (
        <>
            <Sidebar />
            <main
                className={cn(
                    "min-h-[calc(100vh_-_56px)] bg-zinc-50 dark:bg-zinc-900 transition-[margin-left] ease-in-out duration-300",
                    !settings.disabled && (!getOpenState() ? "lg:ml-[90px]" : "lg:ml-72")
                )}
            >
                <Navbar />
                <div className="container pt-8 pb-8 px-4 sm:px-8">
                    {children}
                </div>
            </main>
            <footer
                className={cn(
                    "transition-[margin-left] ease-in-out duration-300",
                    !settings.disabled && (!getOpenState() ? "lg:ml-[90px]" : "lg:ml-72")
                )}
            >
                <Footer />
            </footer>
        </>
    );
}
