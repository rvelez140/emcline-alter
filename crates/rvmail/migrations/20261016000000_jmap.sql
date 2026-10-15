-- Cuentas JMAP (RFC 8620/8621) además de IMAP.
ALTER TABLE "mail_accounts" ADD COLUMN "protocol" text DEFAULT 'imap' NOT NULL;
-- URL de la sesión JMAP (p. ej. https://mail.dominio.com/.well-known/jmap)
ALTER TABLE "mail_accounts" ADD COLUMN "jmap_url" text;

-- Id del mensaje en el servidor JMAP (en IMAP se usa uid)
ALTER TABLE "messages" ADD COLUMN "remote_id" text;
CREATE INDEX "messages_folder_remote_idx" ON "messages" ("folder_id", "remote_id") WHERE "remote_id" IS NOT NULL;
