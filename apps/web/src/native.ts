/**
 * Integración con la app de escritorio/móvil (Tauri 2). En el navegador todo
 * cae a las APIs web estándar, así que la misma web sirve para las dos.
 */

type TauriGlobal = {
  core: { invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
};

function tauri(): TauriGlobal | null {
  const w = window as unknown as { __TAURI__?: TauriGlobal };
  return w.__TAURI__ ?? null;
}

/** ¿Corre dentro de la app RV Mail (Tauri)? */
export const isNativeApp = () => tauri() !== null;

/** Aviso del sistema: nativo en la app, Web Notification en el navegador. */
export function notify(title: string, body: string) {
  const t = tauri();
  if (t) {
    void t.core.invoke('notify', { title, body }).catch(() => undefined);
    return;
  }
  if ('Notification' in window && Notification.permission === 'granted') {
    new Notification(title, { body, icon: '/icon-192.png' });
  }
}

/** Número de no leídos en la bandeja del sistema / icono de la app. */
export function setUnread(count: number) {
  void tauri()
    ?.core.invoke('set_unread', { count })
    .catch(() => undefined);
}

/**
 * Inicio de sesión con Google/Microsoft/Yahoo desde la app: se abre en el
 * navegador del sistema (Google bloquea OAuth dentro de webviews) y la app
 * vuelve con `rvmail://` para terminar la sesión aquí.
 */
export async function nativeSsoLogin(provider: string): Promise<void> {
  const t = tauri();
  if (!t) throw new Error('No es la app nativa');
  await t.core.invoke('sso_login', { provider });
}

/** `mailto:ana@x.com?subject=Hola&cc=b@x.com&body=…` (RFC 6068) → borrador. */
export function parseMailto(raw: string): { to: string; cc?: string; subject?: string; body?: string } | null {
  if (!raw.toLowerCase().startsWith('mailto:')) return null;
  const rest = raw.slice(7);
  const [addr, query = ''] = rest.split('?', 2);
  const params = new URLSearchParams(query.replace(/\+/g, '%2B'));
  const get = (k: string) => {
    for (const [key, v] of params) if (key.toLowerCase() === k) return v;
    return undefined;
  };
  const to = [decodeURIComponent(addr), get('to')].filter(Boolean).join(', ');
  return { to, cc: get('cc'), subject: get('subject'), body: get('body') };
}

/** Registra RV Mail como manejador de enlaces `mailto:` del navegador (PWA). */
export function registerMailtoHandler() {
  try {
    navigator.registerProtocolHandler('mailto', `${location.origin}/?mailto=%s`);
  } catch {
    // Algunos navegadores (Safari, iOS) no lo permiten
  }
}
