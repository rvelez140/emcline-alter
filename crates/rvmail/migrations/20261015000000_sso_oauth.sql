-- SSO (OpenID Connect) e identidades vinculadas, y cuentas de correo por OAuth2.

CREATE TABLE "user_identities" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"user_id" uuid NOT NULL REFERENCES "users"("id") ON DELETE cascade,
	-- google | microsoft | yahoo
	"provider" text NOT NULL,
	-- "sub" del proveedor (en Microsoft: tid + oid, estable entre aplicaciones)
	"subject" text NOT NULL,
	"email" text,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"last_login_at" timestamp with time zone
);
CREATE UNIQUE INDEX "user_identities_provider_subject_uq" ON "user_identities" ("provider", "subject");
CREATE INDEX "user_identities_user_idx" ON "user_identities" ("user_id");

-- Proveedor OAuth de la cuenta (auth_type = 'oauth2'); secret_enc guarda el refresh token cifrado
ALTER TABLE "mail_accounts" ADD COLUMN "oauth_provider" text;

-- Los usuarios creados por SSO no tienen contraseña local
ALTER TABLE "users" ALTER COLUMN "password_hash" DROP NOT NULL;
