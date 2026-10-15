/**
 * Muchos servidores (y GreenMail en desarrollo) no anuncian SPECIAL-USE.
 * Se deduce por nombre, en español e inglés.
 */
const BY_NAME: Array<[RegExp, string]> = [
  [/^inbox$/i, '\\Inbox'],
  [/^(sent|sent items|sent messages|enviados|elementos enviados|mensajes enviados)$/i, '\\Sent'],
  [/^(drafts|borradores)$/i, '\\Drafts'],
  [/^(trash|deleted|deleted items|deleted messages|papelera|elementos eliminados)$/i, '\\Trash'],
  [/^(junk|spam|junk e-?mail|correo no deseado|no deseado)$/i, '\\Junk'],
  [/^(archive|archives|archivo|archivados)$/i, '\\Archive'],
];

export function resolveSpecialUse(path: string, name: string, specialUse?: string | null): string | null {
  if (specialUse) return specialUse;
  if (path.toUpperCase() === 'INBOX') return '\\Inbox';
  const leaf = name.trim();
  for (const [re, use] of BY_NAME) if (re.test(leaf)) return use;
  return null;
}

/** Orden de sincronización: lo que más importa al usuario primero. */
export function folderPriority(specialUse: string | null): number {
  switch (specialUse) {
    case '\\Inbox':
      return 0;
    case '\\Sent':
      return 1;
    case '\\Drafts':
      return 2;
    case '\\Archive':
      return 3;
    case '\\Junk':
      return 8;
    case '\\Trash':
      return 9;
    default:
      return 5;
  }
}

/** Carpetas virtuales que duplican mensajes (Gmail) y no se sincronizan. */
export const SKIP_SPECIAL = new Set(['\\All', '\\Flagged', '\\Important']);
