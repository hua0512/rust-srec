import { create } from 'zustand';

const SIDEBAR_COOKIE_NAME = 'sidebar:state';
const SIDEBAR_COOKIE_MAX_AGE = 60 * 60 * 24 * 365; // 1 year

// Helper to get cookie value (client-side only)
function getCookie(name: string): string | undefined {
    if (typeof document === 'undefined') return undefined;
    const value = `; ${document.cookie}`;
    const parts = value.split(`; ${name}=`);
    if (parts.length === 2) return parts.pop()?.split(';').shift();
    return undefined;
}

// Helper to set cookie
function setCookie(name: string, value: string, maxAge: number) {
    if (typeof document === 'undefined') return;
    document.cookie = `${name}=${value}; path=/; max-age=${maxAge}; SameSite=Lax`;
}

type SidebarState = {
    open: boolean;
    setOpen: (open: boolean | ((prevState: boolean) => boolean)) => void;
    toggleSidebar: () => void;
    openMobile: boolean;
    setOpenMobile: (open: boolean) => void;
    toggleMobileSidebar: () => void;
    _hydrated: boolean;
    _hydrate: (defaultOpen?: boolean) => void;
};

export const useSidebarStore = create<SidebarState>()((set, get) => ({
    // Start with true (expanded) as default - will be corrected on client hydration
    open: true,
    setOpen: (open) => {
        const newOpen = typeof open === 'function' ? open(get().open) : open;
        set({ open: newOpen });
        // Persist to cookie
        setCookie(SIDEBAR_COOKIE_NAME, String(newOpen), SIDEBAR_COOKIE_MAX_AGE);
    },
    toggleSidebar: () => {
        const newOpen = !get().open;
        set({ open: newOpen });
        setCookie(SIDEBAR_COOKIE_NAME, String(newOpen), SIDEBAR_COOKIE_MAX_AGE);
    },

    openMobile: false,
    setOpenMobile: (openMobile) => set({ openMobile }),
    toggleMobileSidebar: () => set((state) => ({ openMobile: !state.openMobile })),

    // Hydration flag
    _hydrated: false,
    _hydrate: (defaultOpen = true) => {
        if (get()._hydrated) return;

        const cookieValue = getCookie(SIDEBAR_COOKIE_NAME);
        const openFromCookie = cookieValue === undefined ? defaultOpen : cookieValue === 'true';

        set({
            open: openFromCookie,
            _hydrated: true
        });
    },
}));
