import { useStore } from "@/hooks/use-store";
import { cn } from "@/lib/utils";
import { Footer } from "./footer";
import { useSidebar } from "@/store/sidebar";
import { useShallow } from "zustand/react/shallow";
import { Sidebar } from "./sidebar";
import { Navbar } from "./navbar";

export default function DashboardPanelLayout({
    children
}: {
    children: React.ReactNode;
}) {
    const sidebar = useStore(
        useSidebar,
        useShallow((state) => ({
            isOpen: state.isOpen || (state.settings.isHoverOpen && state.isHover),
            settings: state.settings,
        }))
    );
    if (!sidebar) return null;
    const { isOpen, settings } = sidebar;
    return (
        <>
            <Sidebar />
            <main
                className={cn(
                    "min-h-[calc(100vh_-_56px)] bg-background transition-[margin-left] ease-in-out duration-300",
                    !settings.disabled && (!isOpen ? "lg:ml-[90px]" : "lg:ml-72")
                )}
            >
                <Navbar />
                <div className="w-full pt-8 pb-8 px-4 sm:px-8">
                    {children}
                </div>
            </main>
            <footer
                className={cn(
                    "transition-[margin-left] ease-in-out duration-300",
                    !settings.disabled && (!isOpen ? "lg:ml-[90px]" : "lg:ml-72")
                )}
            >
                <Footer />
            </footer>
        </>
    );
}
