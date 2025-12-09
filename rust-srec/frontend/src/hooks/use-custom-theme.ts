import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { useEffect } from 'react';

interface CustomThemeState {
    customCss: string;
    setCustomCss: (css: string) => void;
    isEnabled: boolean;
    setIsEnabled: (enabled: boolean) => void;
}

export const useCustomThemeStore = create<CustomThemeState>()(
    persist(
        (set) => ({
            customCss: '',
            setCustomCss: (css) => set({ customCss: css }),
            isEnabled: false,
            setIsEnabled: (enabled) => set({ isEnabled: enabled }),
        }),
        {
            name: 'custom-theme-storage',
        }
    )
);

export function useCustomTheme() {
    const { customCss, setCustomCss, isEnabled, setIsEnabled } = useCustomThemeStore();

    useEffect(() => {
        // Remove existing style tag if it exists
        const existingStyle = document.getElementById('custom-theme-style');
        if (existingStyle) {
            existingStyle.remove();
        }

        if (isEnabled && customCss) {
            const style = document.createElement('style');
            style.id = 'custom-theme-style';
            // Wrap in :root selector if not already present, basic heuristic
            // Actually, Tweakcn exports variables usually as:
            // :root { --background: ... }
            // .dark { --background: ... }
            // So we can inject it directly.
            style.textContent = customCss;
            document.head.appendChild(style);
        }
    }, [customCss, isEnabled]);

    return { customCss, setCustomCss, isEnabled, setIsEnabled };
}
