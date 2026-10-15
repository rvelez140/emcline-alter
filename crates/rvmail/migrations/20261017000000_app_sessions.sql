-- Sesiones de las apps nativas (escritorio/móvil): un registro por dispositivo.
-- El refresh token se guarda solo como hash SHA-256 y rota en cada uso; si se
-- presenta el anterior (ya rotado) se considera robado y se revoca la sesión.
create table app_sessions (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references users(id) on delete cascade,
  device_name text not null,
  platform text not null default 'desconocida',
  refresh_hash text not null unique,
  prev_refresh_hash text,
  user_agent text,
  ip text,
  created_at timestamptz not null default now(),
  last_used_at timestamptz not null default now(),
  expires_at timestamptz not null,
  revoked_at timestamptz
);
create index app_sessions_user_idx on app_sessions(user_id) where revoked_at is null;
create index app_sessions_prev_idx on app_sessions(prev_refresh_hash) where prev_refresh_hash is not null;
