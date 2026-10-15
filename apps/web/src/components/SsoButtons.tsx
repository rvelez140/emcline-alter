import { SsoProvider, ssoStart } from '../api';
import { isNativeApp, nativeSsoLogin } from '../native';

/** Logotipos simplificados de cada proveedor (SVG en línea, sin peticiones externas). */
function Logo({ id }: { id: SsoProvider['id'] }) {
  if (id === 'google') {
    return (
      <svg width="16" height="16" viewBox="0 0 48 48" aria-hidden>
        <path
          fill="#EA4335"
          d="M24 9.5c3.5 0 6.6 1.2 9 3.6l6.7-6.7C35.6 2.4 30.2 0 24 0 14.6 0 6.6 5.4 2.6 13.3l7.8 6C12.3 13.6 17.7 9.5 24 9.5z"
        />
        <path
          fill="#4285F4"
          d="M46.5 24.5c0-1.6-.1-3.1-.4-4.5H24v9h12.7c-.6 2.9-2.2 5.4-4.7 7.1l7.6 5.9c4.4-4.1 6.9-10.1 6.9-17.5z"
        />
        <path
          fill="#FBBC05"
          d="M10.4 28.7c-.5-1.4-.8-3-.8-4.7s.3-3.3.8-4.7l-7.8-6C.9 16.6 0 20.2 0 24s.9 7.4 2.6 10.7l7.8-6z"
        />
        <path
          fill="#34A853"
          d="M24 48c6.5 0 11.9-2.1 15.9-5.8l-7.6-5.9c-2.1 1.4-4.9 2.3-8.3 2.3-6.3 0-11.7-4.1-13.6-9.9l-7.8 6C6.6 42.6 14.6 48 24 48z"
        />
      </svg>
    );
  }
  if (id === 'microsoft') {
    return (
      <svg width="16" height="16" viewBox="0 0 23 23" aria-hidden>
        <path fill="#f35325" d="M1 1h10v10H1z" />
        <path fill="#81bc06" d="M12 1h10v10H12z" />
        <path fill="#05a6f0" d="M1 12h10v10H1z" />
        <path fill="#ffba08" d="M12 12h10v10H12z" />
      </svg>
    );
  }
  return (
    <span
      className="flex h-4 w-4 items-center justify-center rounded-sm bg-[#6001d2] text-[10px] font-bold text-white"
      aria-hidden
    >
      Y!
    </span>
  );
}

export function SsoButtons({
  providers,
  mode,
  label,
}: {
  providers: SsoProvider[];
  mode: 'login' | 'link' | 'account';
  label: (name: string) => string;
}) {
  if (!providers.length) return null;
  return (
    <div className="space-y-2">
      {providers.map((p) => (
        <button
          key={p.id}
          type="button"
          className="btn w-full border border-slate-300 bg-white text-slate-700 hover:bg-slate-50 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-200 dark:hover:bg-slate-800"
          onClick={() =>
            // En la app de escritorio el inicio de sesión va por el navegador del sistema
            isNativeApp() && mode === 'login'
              ? void nativeSsoLogin(p.id).catch((e: Error) => alert(e.message))
              : ssoStart(p.id, mode)
          }
        >
          <Logo id={p.id} />
          {label(p.name)}
        </button>
      ))}
    </div>
  );
}
