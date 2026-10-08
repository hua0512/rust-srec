import { accountInitials, avatarTone } from '../account-avatar';

describe('accountInitials', () => {
  it('takes the first letters of the first two words', () => {
    expect(accountInitials('Main account')).toBe('MA');
    expect(accountInitials('old phone for travel')).toBe('OP');
    expect(accountInitials('  backup   two ')).toBe('BT');
  });

  it('takes the first two letters of a single word', () => {
    expect(accountInitials('Spare')).toBe('SP');
    expect(accountInitials('x')).toBe('X');
  });

  it('takes one character of a Chinese, Japanese or Korean label', () => {
    expect(accountInitials('主账号')).toBe('主');
    expect(accountInitials('备用 账号')).toBe('备');
    expect(accountInitials('サブ')).toBe('サ');
    expect(accountInitials('부계정')).toBe('부');
  });

  it('keeps a leading emoji whole and alone', () => {
    expect(accountInitials('🎮 Gaming')).toBe('🎮');
    expect(accountInitials('👩‍💻 Work')).toBe('👩‍💻');
  });

  it('gives nothing for a blank label', () => {
    expect(accountInitials('')).toBe('');
    expect(accountInitials('   ')).toBe('');
  });
});

describe('avatarTone', () => {
  it('gives a label the same tint every time', () => {
    expect(avatarTone('Main account')).toBe(avatarTone('Main account'));
    const tones = new Set(
      ['Main account', 'Backup', 'Old phone', 'Spare', 'Proxied'].map(
        avatarTone,
      ),
    );
    expect(tones.size).toBeGreaterThan(1);
  });
});
