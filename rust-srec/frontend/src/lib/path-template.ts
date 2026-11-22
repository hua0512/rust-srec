// Output folders may hold filename placeholders (`{streamer}`, `%Y-%m-%d`),
// which the backend expands per recording. Only the components before the
// first placeholder name a folder that exists on disk.
const PLACEHOLDER = /[{%]/;
const SEPARATOR = /([/\\])/;
const ROOT = /^(?:[/\\]|[a-zA-Z]:[/\\])$/;

interface PathTemplateParts {
  /** Leading components without a placeholder; empty when the first has one. */
  base: string;
  /** The rest of the path from the first placeholder component on, or `''`. */
  suffix: string;
}

export function splitPathTemplate(path: string): PathTemplateParts {
  // Odd indices hold the separators, even indices the components.
  const parts = path.split(SEPARATOR);
  const first = parts.findIndex(
    (part, index) => index % 2 === 0 && PLACEHOLDER.test(part),
  );
  if (first === -1) return { base: path, suffix: '' };

  let base = parts.slice(0, first).join('');
  if (
    base.length > 1 &&
    SEPARATOR.test(base.at(-1) ?? '') &&
    !ROOT.test(base)
  ) {
    base = base.slice(0, -1);
  }
  return { base, suffix: parts.slice(first).join('') };
}

/** Appends a placeholder suffix to a picked folder in that folder's style. */
export function joinPathTemplate(folder: string, suffix: string): string {
  if (!suffix) return folder;
  const separator = folder.includes('\\') && !folder.includes('/') ? '\\' : '/';
  return SEPARATOR.test(folder.at(-1) ?? '')
    ? `${folder}${suffix}`
    : `${folder}${separator}${suffix}`;
}
