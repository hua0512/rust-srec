type SessionOverrides = {
  accessToken?: string;
  /** How long the access token stays valid from now. */
  expiresInMs?: number;
  mustChangePassword?: boolean;
};

/**
 * A signed-in session as `checkAuthFn` returns it. By default its access token
 * is well clear of the renewal window, so a check made with it is reused.
 */
export function testSession({
  accessToken = 'access-token',
  expiresInMs = 10 * 60_000,
  mustChangePassword = false,
}: SessionOverrides = {}) {
  return {
    username: 'user',
    token: {
      access_token: accessToken,
      expires_in: Date.now() + expiresInMs,
      refresh_expires_in: Date.now() + expiresInMs + 60 * 60_000,
    },
    roles: [],
    mustChangePassword,
  };
}
