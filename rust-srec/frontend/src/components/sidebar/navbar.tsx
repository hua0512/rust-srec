import { Link, useLocation } from "@tanstack/react-router";
import React from "react";
import { SheetMenu } from "@/components/sidebar/sheet-menu";
import { ModeToggle } from "./mode-toggle";
import { LanguageSwitcher } from "../language-switcher";
import {
    Breadcrumb,
    BreadcrumbItem,
    BreadcrumbLink,
    BreadcrumbList,
    BreadcrumbPage,
    BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";

interface NavbarProps { }

export function Navbar({ }: NavbarProps) {
    const location = useLocation();
    const pathSegments = location.pathname.split("/").filter(Boolean);

    return (
        <header className="sticky top-0 z-10 w-full bg-background/95 shadow backdrop-blur supports-[backdrop-filter]:bg-background/60 dark:shadow-secondary">
            <div className="mx-4 sm:mx-8 flex h-14 items-center">
                <div className="flex items-center space-x-4 lg:space-x-0">
                    <SheetMenu />
                    <Breadcrumb>
                        <BreadcrumbList>
                            <BreadcrumbItem>
                                <BreadcrumbLink asChild>
                                    <Link to="/dashboard">Home</Link>
                                </BreadcrumbLink>
                            </BreadcrumbItem>
                            {pathSegments.map((segment, index) => {
                                const isLast = index === pathSegments.length - 1;
                                const href = `/${pathSegments.slice(0, index + 1).join("/")}`;
                                return (
                                    <React.Fragment key={href}>
                                        <BreadcrumbSeparator />
                                        <BreadcrumbItem>
                                            {isLast ? (
                                                <BreadcrumbPage className="capitalize">{segment}</BreadcrumbPage>
                                            ) : (
                                                <BreadcrumbLink asChild className="capitalize">
                                                    <Link to={href}>{segment}</Link>
                                                </BreadcrumbLink>
                                            )}
                                        </BreadcrumbItem>
                                    </React.Fragment>
                                );
                            })}
                        </BreadcrumbList>
                    </Breadcrumb>
                </div>
                <div className="flex flex-1 items-center justify-end space-x-4">
                    <LanguageSwitcher />
                    <ModeToggle />
                </div>
            </div>
        </header>
    );
}
