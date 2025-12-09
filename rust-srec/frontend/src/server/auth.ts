import { createServerFn } from '@tanstack/react-start'
import { useAppSession } from '../utils/session'
import { LoginRequestSchema, LoginResponseSchema } from '../api/schemas'
import { z } from 'zod'
import ky from 'ky'

// Use environment variable or default to local API
// Note: In SSR/Server Functions, ensure this env var is available.
const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || 'http://localhost:12555/api'

// Dedicated ky instance for server-side calls (no store dependency)
const serverClient = ky.create({
    prefixUrl: API_BASE_URL,
    timeout: 10000,
})

export const loginFn = createServerFn({ method: "POST" })
    .inputValidator((data: z.infer<typeof LoginRequestSchema>) => data)
    .handler(async ({ data }) => {
        try {
            const json = await serverClient.post('auth/login', { json: data }).json()
            const parsed = LoginResponseSchema.parse(json)

            const session = await useAppSession()

            const userData = {
                username: data.username,
                token: {
                    access_token: parsed.access_token,
                    refresh_token: parsed.refresh_token,
                    expires_in: parsed.expires_in,
                    refresh_expires_in: parsed.refresh_expires_in,
                },
                roles: parsed.roles,
                mustChangePassword: parsed.must_change_password
            }
            await session.update(userData)

            return userData
        } catch (error) {
            console.error('Login failed:', error)
            throw error
        }
    })

export const logoutFn = createServerFn({ method: "POST" })
    .handler(async () => {
        const session = await useAppSession()
        const refreshToken = session.data.token?.refresh_token

        if (refreshToken) {
            try {
                // Best effort logout on backend
                await serverClient.post('auth/logout', {
                    json: { refresh_token: refreshToken }
                })
            } catch (e) {
                console.error('Backend logout failed (ignoring):', e)
            }
        }

        await session.clear()
        return { success: true }
    })

export const checkAuthFn = createServerFn({ method: "POST" })
    .handler(async () => {
        const session = await useAppSession()
        const refreshToken = session.data.token?.refresh_token

        if (!refreshToken) {
            return null
        }

        // check if the refresh token is expired
        const now = Date.now()
        const expiresAt = session.data.token?.refresh_expires_in || 0
        if (now > expiresAt) {
            return null
        }

        try {
            // Attempt to refresh token using the stored refresh token
            const json = await serverClient.post('auth/refresh', {
                json: { refresh_token: refreshToken }
            }).json()

            const parsed = LoginResponseSchema.parse(json)

            // Update session with potentially rotated refresh token and new user info

            const userData = {
                username: session.data.username,
                token: {
                    access_token: parsed.access_token,
                    refresh_token: parsed.refresh_token,
                    expires_in: parsed.expires_in,
                    refresh_expires_in: parsed.refresh_expires_in,
                },
                roles: parsed.roles,
                mustChangePassword: parsed.must_change_password
            }

            await session.update(userData)

            return userData
        } catch (error) {
            // Refresh failed (expired/invalid), clear session
            console.warn('Token refresh failed, clearing session:', error)
            await session.clear()
            return null
        }
    })
