import { runMigrations } from './index';

runMigrations()
  .then(() => {
    console.log('Migraciones aplicadas');
  })
  .catch((err) => {
    console.error('Error aplicando migraciones:', err);
    process.exit(1);
  });
