import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { useEffect } from 'react';

export type ThemeRadius = 0 | 0.3 | 0.5 | 0.625 | 0.75 | 1.0;

interface ThemeRadiusState {
    radius: number;
    setRadius: (radius: number) => void;
}

export const useThemeRadiusStore = create<ThemeRadiusState>()(
    persist(
        (set) => ({
            radius: 0.625,
            setRadius: (radius) => set({ radius }),
        }),
        {
            name: 'theme-radius-storage',
        }
    )
);

export function useThemeRadius() {
    const { radius, setRadius } = useThemeRadiusStore();

    useEffect(() => {
        document.body.style.setProperty('--radius', `${radius}rem`);
    }, [radius]);

    return { radius, setRadius };
}
