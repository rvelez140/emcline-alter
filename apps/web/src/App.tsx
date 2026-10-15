import { useQuery } from '@tanstack/react-query';
import { api, ApiError } from './api';
import { AuthScreen } from './components/AuthScreen';
import { MailApp } from './components/MailApp';

export function App() {
  const setup = useQuery({ queryKey: ['setup'], queryFn: api.setup });
  const me = useQuery({
    queryKey: ['me'],
    queryFn: api.me,
    retry: (count, err) => !(err instanceof ApiError && err.status === 401) && count < 2,
  });

  if (setup.isLoading || me.isLoading) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-slate-500">Cargando RV Mail…</div>
    );
  }
  if (setup.isError) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-center text-sm text-red-600">
        No se pudo conectar con el servidor. Revisa tu conexión e inténtalo de nuevo.
      </div>
    );
  }
  if (!me.data) return <AuthScreen mode={setup.data?.needsSetup ? 'setup' : 'login'} />;
  return <MailApp user={me.data} />;
}
