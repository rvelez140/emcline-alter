import { useQueryClient } from '@tanstack/react-query';
import { Mail } from 'lucide-react';
import { FormEvent, useState } from 'react';
import { api } from '../api';

export function AuthScreen({ mode, mailboxLogin }: { mode: 'setup' | 'login'; mailboxLogin?: boolean }) {
  const qc = useQueryClient();
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const user = mode === 'setup' ? await api.register(name, email, password) : await api.login(email, password);
      qc.setQueryData(['me'], user);
      await qc.invalidateQueries({ queryKey: ['setup'] });
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full items-center justify-center p-4">
      <form
        onSubmit={submit}
        className="w-full max-w-sm space-y-4 rounded-2xl border border-slate-200 bg-white p-6 shadow-sm dark:border-slate-800 dark:bg-slate-900"
      >
        <div className="flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-xl bg-brand-600 text-white">
            <Mail size={20} />
          </div>
          <div>
            <h1 className="text-lg font-semibold">RV Mail</h1>
            <p className="text-xs text-slate-500">Soluciones RV</p>
          </div>
        </div>

        {mode === 'setup' && (
          <p className="rounded-lg bg-brand-50 p-3 text-xs text-brand-700 dark:bg-brand-700/20 dark:text-brand-100">
            Primera vez: crea la cuenta de administrador de esta instalación.
          </p>
        )}

        {mode === 'setup' && (
          <div>
            <label className="label" htmlFor="name">Nombre</label>
            <input id="name" className="input" value={name} onChange={(e) => setName(e.target.value)} required />
          </div>
        )}
        <div>
          <label className="label" htmlFor="email">Correo</label>
          <input
            id="email"
            type="email"
            autoComplete="username"
            className="input"
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            required
          />
        </div>
        <div>
          <label className="label" htmlFor="password">Contraseña</label>
          <input
            id="password"
            type="password"
            autoComplete={mode === 'setup' ? 'new-password' : 'current-password'}
            minLength={mode === 'setup' ? 10 : 1}
            className="input"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
          />
        </div>

        {mode === 'login' && mailboxLogin && (
          <p className="text-xs text-slate-500">
            ¿Tienes un buzón en este servidor? Entra con tu correo y la contraseña del buzón.
          </p>
        )}

        {error && <p className="text-sm text-red-600">{error}</p>}

        <button className="btn-primary w-full" disabled={busy}>
          {busy ? 'Un momento…' : mode === 'setup' ? 'Crear administrador' : 'Entrar'}
        </button>
      </form>
    </div>
  );
}
