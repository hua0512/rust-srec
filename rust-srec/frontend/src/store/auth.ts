import { create } from 'zustand';
import { persist, createJSONStorage, StateStorage } from 'zustand/middleware';

interface AuthState {
    accessToken: string | null;
    refreshToken: string | null;
    user: {
        roles: string[];
        must_change_password: boolean;
    } | null;
    isAuthenticated: boolean;
    remember: boolean;

    login: (accessToken: string, refreshToken: string, roles: string[], mustChangePassword: boolean, remember: boolean) => void;
    logout: () => void;
    updatePasswordChanged: () => void;
}

const customStorage: StateStorage = {
    getItem: (name: string): string | null => {
        // if (typeof window === 'undefined') return null;
        // Try localStorage first (remembered sessions)
        const local = localStorage.getItem(name);
        if (local) return local;
        // Then try sessionStorage (temporary sessions)
        return sessionStorage.getItem(name);
    },
    setItem: (name: string, value: string): void => {
        // if (typeof window === 'undefined') return;
        try {
            const parsed = JSON.parse(value);
            const remember = parsed.state?.remember;

            if (remember) {
                localStorage.setItem(name, value);
                sessionStorage.removeItem(name);
            } else {
                sessionStorage.setItem(name, value);
                localStorage.removeItem(name);
            }
        } catch (e) {
            console.error('Failed to parse auth state:', e);
            // Fallback to localStorage if parsing fails
            localStorage.setItem(name, value);
        }
    },
    removeItem: (name: string): void => {
        // if (typeof window === 'undefined') return;
        localStorage.removeItem(name);
        sessionStorage.removeItem(name);
    },
};

export const useAuthStore = create<AuthState>()(
    persist(
        (set) => ({
            accessToken: null,
            refreshToken: null,
            user: null,
            isAuthenticated: false,
            remember: false,

            login: (accessToken, refreshToken, roles, mustChangePassword, remember) =>
                set({
                    accessToken,
                    refreshToken,
                    user: { roles, must_change_password: mustChangePassword },
                    isAuthenticated: true,
                    remember,
                }),

            logout: () => {
                console.log('Auth Store: logout called');
                set({
                    accessToken: null,
                    refreshToken: null,
                    user: null,
                    isAuthenticated: false,
                    remember: false,
                });
            },

            updatePasswordChanged: () =>
                set((state) => ({
                    user: state.user ? { ...state.user, must_change_password: false } : null,
                })),
        }),
        {
            name: 'auth-storage',
            storage: createJSONStorage(() => customStorage),
        }
    )
);
