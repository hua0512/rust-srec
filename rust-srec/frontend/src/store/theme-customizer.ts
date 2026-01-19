import { create } from 'zustand';

type ThemeCustomizerState = {
  isOpen: boolean;
  setIsOpen: (isOpen: boolean) => void;
};

export const useThemeCustomizer = create<ThemeCustomizerState>((set) => ({
  isOpen: false,
  setIsOpen: (isOpen) => set({ isOpen }),
}));
