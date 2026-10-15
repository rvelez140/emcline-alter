import { createCipheriv, createDecipheriv, randomBytes } from 'node:crypto';

/**
 * Cifrado de credenciales de cuentas de correo (AES-256-GCM).
 * Formato almacenado: "v1:<iv b64>:<tag b64>:<ciphertext b64>".
 * La clave viene de MASTER_KEY (32 bytes en base64).
 */
const VERSION = 'v1';

function loadKey(masterKey: string | undefined): Buffer {
  if (!masterKey) throw new Error('MASTER_KEY no está definida');
  const key = Buffer.from(masterKey, 'base64');
  if (key.length !== 32) throw new Error('MASTER_KEY debe ser de 32 bytes en base64');
  return key;
}

export function encryptSecret(plain: string, masterKey = process.env.MASTER_KEY): string {
  const key = loadKey(masterKey);
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', key, iv);
  const enc = Buffer.concat([cipher.update(plain, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return [VERSION, iv.toString('base64'), tag.toString('base64'), enc.toString('base64')].join(':');
}

export function decryptSecret(stored: string, masterKey = process.env.MASTER_KEY): string {
  const key = loadKey(masterKey);
  const [version, ivB64, tagB64, dataB64] = stored.split(':');
  if (version !== VERSION || !ivB64 || !tagB64 || !dataB64) {
    throw new Error('Formato de secreto cifrado no reconocido');
  }
  const decipher = createDecipheriv('aes-256-gcm', key, Buffer.from(ivB64, 'base64'));
  decipher.setAuthTag(Buffer.from(tagB64, 'base64'));
  return Buffer.concat([decipher.update(Buffer.from(dataB64, 'base64')), decipher.final()]).toString('utf8');
}
