import { useSession } from '@tanstack/react-start/server'

export type SessionData = {
    username: string,
    token: {
        access_token: string,
        refresh_token: string,
        // Stored as absolute timestamps (ms since epoch)
        expires_in: number,
        refresh_expires_in: number,
    },
    roles: string[]
    mustChangePassword: boolean
}

export function useAppSession() {
    return useSession<SessionData>({
        name: 'srec_session',
        password: process.env.SESSION_SECRET || 'dev_secret_must_be_at_least_32_chars_long_and_random',
        cookie: {
            secure: process.env.NODE_ENV === 'production',
            sameSite: 'lax',
            httpOnly: true,
            maxAge: 30 * 24 * 60 * 60, // 30 days
        },
    })
}
