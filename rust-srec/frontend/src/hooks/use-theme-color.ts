import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { useEffect } from 'react';

export type ThemeColor = 'zinc' | 'red' | 'rose' | 'orange' | 'green' | 'blue' | 'yellow' | 'violet';

interface ThemeColorState {
    themeColor: ThemeColor;
    setThemeColor: (color: ThemeColor) => void;
}

export const useThemeColorStore = create<ThemeColorState>()(
    persist(
        (set) => ({
            themeColor: 'zinc',
            setThemeColor: (color) => set({ themeColor: color }),
        }),
        {
            name: 'theme-color-storage',
        }
    )
);

export function useThemeColor() {
    const { themeColor, setThemeColor } = useThemeColorStore();

    useEffect(() => {
        const body = document.body;
        body.setAttribute('data-theme-color', themeColor);
    }, [themeColor]);

    return { themeColor, setThemeColor };
}
