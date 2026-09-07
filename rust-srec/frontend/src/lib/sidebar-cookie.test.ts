import { readSidebarCookie, SIDEBAR_COOKIE_KEY } from './sidebar-cookie';

describe('readSidebarCookie', () => {
  it('reads the state from either end of a cookie string', () => {
    expect(
      readSidebarCookie(`${SIDEBAR_COOKIE_KEY}=false; theme_mode=dark`),
    ).toBe(false);
    expect(
      readSidebarCookie(`theme_mode=dark; ${SIDEBAR_COOKIE_KEY}=true`),
    ).toBe(true);
  });

  it('reports no preference rather than a default', () => {
    expect(readSidebarCookie(undefined)).toBeUndefined();
    expect(readSidebarCookie('')).toBeUndefined();
    expect(readSidebarCookie('theme_mode=dark')).toBeUndefined();
    expect(readSidebarCookie(`${SIDEBAR_COOKIE_KEY}=maybe`)).toBeUndefined();
  });

  it('does not match a cookie whose name merely ends with this one', () => {
    expect(
      readSidebarCookie(`old_${SIDEBAR_COOKIE_KEY}=false`),
    ).toBeUndefined();
  });
});
