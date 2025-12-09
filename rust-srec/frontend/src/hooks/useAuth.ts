import { useAuthStore } from '../store/auth';
import { authApi } from '../api/endpoints';
import { LoginRequestSchema, ChangePasswordRequestSchema } from '../api/schemas';
import { z } from 'zod';
import { toast } from 'sonner';
import { useNavigate } from '@tanstack/react-router';

export const useAuth = () => {
    const store = useAuthStore();
    const navigate = useNavigate();

    const login = async (data: z.infer<typeof LoginRequestSchema> & { remember?: boolean }) => {
        try {
            const response = await authApi.login(data);
            store.login(
                response.access_token,
                response.refresh_token,
                response.roles,
                response.must_change_password,
                data.remember ?? false
            );

            if (response.must_change_password) {
                toast.warning('Password change required');
                navigate({ to: '/change-password' });
            } else {
                toast.success('Logged in successfully');
                navigate({ to: '/dashboard' });
            }
        } catch (error: any) {
            toast.error(error.message || 'Login failed');
            throw error;
        }
    };

    const logout = async () => {
        try {
            if (store.refreshToken) {
                await authApi.logout(store.refreshToken);
            }
        } catch (error) {
            console.error('Logout failed', error);
        } finally {
            store.logout();
            navigate({ to: '/login' });
            toast.success('Logged out');
        }
    };

    const changePassword = async (data: z.infer<typeof ChangePasswordRequestSchema>) => {
        try {
            await authApi.changePassword(data);
            store.updatePasswordChanged();
            toast.success('Password changed successfully');
            navigate({ to: '/dashboard' });
        } catch (error: any) {
            toast.error(error.message || 'Password change failed');
            throw error;
        }
    };

    return {
        ...store,
        login,
        logout,
        changePassword,
    };
};
