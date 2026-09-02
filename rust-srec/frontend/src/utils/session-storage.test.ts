import { parseStoredSession } from './session-storage';
import {
  resolveCookieSecurity,
  resolveSessionSecret,
  warnInsecureCookieOnce,
} from './session.server';

describe('parseStoredSession', () => {
  it('returns an object from valid JSON', () => {
    expect(parseStoredSession('{"username":"user"}')).toEqual({
      username: 'user',
    });
  });

  it.each([null, '', 'invalid', '[]', 'null'])(
    'rejects non-session input %p',
    (raw) => {
      expect(parseStoredSession(raw)).toEqual({});
    },
  );
});

describe('resolveSessionSecret', () => {
  it('uses the configured secret', () => {
    expect(resolveSessionSecret('configured-secret', 'production')).toBe(
      'configured-secret',
    );
  });

  it('allows the built-in secret outside production', () => {
    expect(resolveSessionSecret(undefined, 'development')).toContain(
      'dev_secret',
    );
  });

  it('rejects a missing production secret', () => {
    expect(() => resolveSessionSecret(undefined, 'production')).toThrow(
      'SESSION_SECRET must be set in production',
    );
  });
});

describe('resolveCookieSecurity', () => {
  it.each([
    ['https', true],
    ['HTTPS', true],
    ['https, http', true],
    ['http, https', false],
    ['http', false],
  ])('reads X-Forwarded-Proto %p', (forwardedProto, secure) => {
    expect(resolveCookieSecurity({ forwardedProto })).toEqual({
      secure,
      warnInsecure: false,
    });
  });

  it.each([
    ['for=192.0.2.60;proto=https;by=203.0.113.43', true],
    ['proto="https"', true],
    ['proto=HTTPS, proto=http', true],
    ['for=192.0.2.60;proto=http', false],
    ['for=192.0.2.60', false],
  ])('falls back to the Forwarded header %p', (forwarded, secure) => {
    expect(resolveCookieSecurity({ forwarded }).secure).toBe(secure);
  });

  it('prefers X-Forwarded-Proto over Forwarded', () => {
    expect(
      resolveCookieSecurity({
        forwardedProto: 'https',
        forwarded: 'proto=http',
      }).secure,
    ).toBe(true);
  });

  it('assumes HTTPS in production when no proxy header is present', () => {
    expect(resolveCookieSecurity({ nodeEnv: 'production' })).toEqual({
      secure: true,
      warnInsecure: false,
    });
  });

  it('stays insecure outside production when no proxy header is present', () => {
    expect(resolveCookieSecurity({ nodeEnv: 'development' })).toEqual({
      secure: false,
      warnInsecure: false,
    });
  });

  it('flags a plain-HTTP production request', () => {
    expect(
      resolveCookieSecurity({ forwardedProto: 'http', nodeEnv: 'production' }),
    ).toEqual({ secure: false, warnInsecure: true });
  });

  it('lets COOKIE_SECURE=false win over an https proxy header', () => {
    expect(
      resolveCookieSecurity({
        cookieSecure: 'false',
        forwardedProto: 'https',
        nodeEnv: 'production',
      }),
    ).toEqual({ secure: false, warnInsecure: false });
  });

  it('lets COOKIE_SECURE=true win over a plain-HTTP request', () => {
    expect(
      resolveCookieSecurity({ cookieSecure: 'TRUE', forwardedProto: 'http' })
        .secure,
    ).toBe(true);
  });

  it('ignores an unrecognised COOKIE_SECURE value', () => {
    expect(
      resolveCookieSecurity({ cookieSecure: 'yes', forwardedProto: 'https' })
        .secure,
    ).toBe(true);
  });
});

describe('warnInsecureCookieOnce', () => {
  it('logs at most once per process', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    warnInsecureCookieOnce();
    warnInsecureCookieOnce();

    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });
});
