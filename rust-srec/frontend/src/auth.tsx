import * as React from 'react'
import { useAuthStore } from './store/auth'
import { authApi } from './api/endpoints'
import { toast } from 'sonner'
import { useRouter } from '@tanstack/react-router'

export interface AuthContextType {
    isAuthenticated: boolean
    user: {
        roles: string[];
        must_change_password: boolean;
    } | null
    login: (accessToken: string, refreshToken: string, roles: string[], mustChangePassword: boolean, remember: boolean) => void
    logout: (refreshToken?: string | null) => Promise<void>
}

const AuthContext = React.createContext<AuthContextType | null>(null)

export function AuthProvider({ children }: { children: React.ReactNode }) {
    const store = useAuthStore()
    const router = useRouter()

    const logout = async () => {
        try {
            if (store.refreshToken) {
                await authApi.logout(store.refreshToken)
            }
        } catch (error) {
            console.error('Logout failed', error)
        } finally {
            store.logout()
            toast.success('Logged out')
            router.invalidate()
        }
    }

    // Maps store state to context
    const value: AuthContextType = {
        isAuthenticated: store.isAuthenticated,
        user: store.user,
        login: store.login,
        logout: logout,
    }

    return (
        <AuthContext.Provider value={value}>
            {children}
        </AuthContext.Provider>
    )
}

export function useAuth() {
    const context = React.useContext(AuthContext)
    if (!context) {
        throw new Error('useAuth must be used within an AuthProvider')
    }
    return context
}
