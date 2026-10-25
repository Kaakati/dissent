// The passphrase rules of the portable profile screens. Ported from views/unlock.js. Only the minimum length is
// enforced; the hint is advice.
export const MIN_PASSPHRASE = 8;

export type StrengthLevel = 'short' | 'weak' | 'fair' | 'strong';

export function strengthHint(pass: string): { level: StrengthLevel; text: string } {
  if (pass.length < MIN_PASSPHRASE) return { level: 'short', text: `Use at least ${MIN_PASSPHRASE} characters.` };
  const kinds = [/[a-z]/, /[A-Z]/, /\d/, /[^A-Za-z0-9]/].filter((re) => re.test(pass)).length;
  if (pass.length >= 16 || (pass.length >= 12 && kinds >= 3)) return { level: 'strong', text: 'Strong passphrase.' };
  if (pass.length >= 12 || kinds >= 3) return { level: 'fair', text: 'Fair. A longer passphrase is safer.' };
  return { level: 'weak', text: 'Weak. Add length or more kinds of character.' };
}
