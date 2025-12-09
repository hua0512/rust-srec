import ky from 'ky';
import { useAuthStore } from '../store/auth';
import { LoginResponseSchema } from './schemas';



// Create a ky instance with default settings
// NOTE: To prevent memory leaks with Ky (Issue #797), always ensure response bodies
// are consumed (e.g., .json(), .text()) or cancelled, even for void endpoints.
export const apiClient = ky.create({
    prefixUrl: import.meta.env.VITE_API_BASE_URL || '/api',
    timeout: 10000,
    retry: {
        limit: 2,
        methods: ['get'],
        statusCodes: [408, 413, 429, 500, 502, 503, 504],
    },
    hooks: {
        beforeRequest: [
            (request) => {
                const token = useAuthStore.getState().accessToken;
                if (token) {
                    request.headers.set('Authorization', `Bearer ${token}`);
                }
            },
        ],
        afterResponse: [
            async (request, _options, response) => {
                if (response.status === 401) {
                    console.log('API Client: Intercepted 401 response from', request.url);
                    const { refreshToken, logout, login } = useAuthStore.getState();

                    if (!refreshToken) {
                        logout();
                        return;
                    }

                    // Try to refresh the token
                    try {
                        // Use a fresh ky instance to avoid infinite loops
                        const refreshResponse = await ky.post(`${import.meta.env.VITE_API_BASE_URL || '/api'}/auth/refresh`, {
                            json: { refresh_token: refreshToken },
                        }).json();

                        const data = LoginResponseSchema.parse(refreshResponse);

                        // Update the store
                        login(
                            data.access_token,
                            data.refresh_token,
                            data.roles,
                            data.must_change_password,
                            useAuthStore.getState().remember
                        );

                        // Retry the original request with the new token
                        request.headers.set('Authorization', `Bearer ${data.access_token}`);
                        return apiClient(request);
                    } catch (error) {
                        console.error('Refresh token failed:', error);
                        // Refresh failed, logout
                        logout();
                    }
                }
            },
        ],
    },
});

// Helper to handle API errors
export class ApiError extends Error {
    constructor(public status: number, public message: string, public data?: any) {
        super(message);
        this.name = 'ApiError';
    }
}

export const handleApiError = async (error: any) => {
    console.log(error);
    if (error.name === 'HTTPError') {
        const json = await error.response.json().catch(() => ({}));
        throw new ApiError(error.response.status, json.message || error.message, json);
    }
    throw error;
};
