import { sql } from 'drizzle-orm';
import {
  bigint,
  boolean,
  customType,
  index,
  integer,
  jsonb,
  pgTable,
  text,
  timestamp,
  uniqueIndex,
  uuid,
} from 'drizzle-orm/pg-core';

const tsvector = customType<{ data: string }>({ dataType: () => 'tsvector' });

export type Address = { name?: string | null; address: string };

export const users = pgTable('users', {
  id: uuid('id').primaryKey().defaultRandom(),
  email: text('email').notNull().unique(),
  name: text('name').notNull(),
  passwordHash: text('password_hash').notNull(),
  role: text('role', { enum: ['admin', 'user'] }).notNull().default('user'),
  createdAt: timestamp('created_at', { withTimezone: true }).notNull().defaultNow(),
});

export const mailAccounts = pgTable(
  'mail_accounts',
  {
    id: uuid('id').primaryKey().defaultRandom(),
    userId: uuid('user_id')
      .notNull()
      .references(() => users.id, { onDelete: 'cascade' }),
    displayName: text('display_name').notNull(),
    email: text('email').notNull(),
    color: text('color').notNull().default('#2563eb'),
    imapHost: text('imap_host').notNull(),
    imapPort: integer('imap_port').notNull().default(993),
    imapSecure: boolean('imap_secure').notNull().default(true),
    smtpHost: text('smtp_host').notNull(),
    smtpPort: integer('smtp_port').notNull().default(465),
    smtpSecure: boolean('smtp_secure').notNull().default(true),
    username: text('username').notNull(),
    /** Contraseña o token cifrado con AES-256-GCM (ver @rvmail/shared/crypto). */
    secretEnc: text('secret_enc').notNull(),
    authType: text('auth_type', { enum: ['password', 'oauth2'] }).notNull().default('password'),
    signatureHtml: text('signature_html'),
    enabled: boolean('enabled').notNull().default(true),
    status: text('status').notNull().default('pending'),
    lastError: text('last_error'),
    lastSyncAt: timestamp('last_sync_at', { withTimezone: true }),
    sortOrder: integer('sort_order').notNull().default(0),
    createdAt: timestamp('created_at', { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [index('mail_accounts_user_idx').on(t.userId)],
);

export const folders = pgTable(
  'folders',
  {
    id: uuid('id').primaryKey().defaultRandom(),
    accountId: uuid('account_id')
      .notNull()
      .references(() => mailAccounts.id, { onDelete: 'cascade' }),
    path: text('path').notNull(),
    name: text('name').notNull(),
    delimiter: text('delimiter'),
    /** \Inbox, \Sent, \Drafts, \Trash, \Junk, \Archive o null */
    specialUse: text('special_use'),
    subscribed: boolean('subscribed').notNull().default(true),
    uidValidity: bigint('uid_validity', { mode: 'number' }),
    lastUid: bigint('last_uid', { mode: 'number' }).notNull().default(0),
    highestModseq: text('highest_modseq'),
    unreadCount: integer('unread_count').notNull().default(0),
    totalCount: integer('total_count').notNull().default(0),
    syncedAt: timestamp('synced_at', { withTimezone: true }),
  },
  (t) => [uniqueIndex('folders_account_path_uq').on(t.accountId, t.path)],
);

export const messages = pgTable(
  'messages',
  {
    id: uuid('id').primaryKey().defaultRandom(),
    accountId: uuid('account_id')
      .notNull()
      .references(() => mailAccounts.id, { onDelete: 'cascade' }),
    folderId: uuid('folder_id')
      .notNull()
      .references(() => folders.id, { onDelete: 'cascade' }),
    uid: bigint('uid', { mode: 'number' }).notNull(),
    messageId: text('message_id'),
    inReplyTo: text('in_reply_to'),
    threadKey: text('thread_key'),
    subject: text('subject').notNull().default(''),
    fromName: text('from_name'),
    fromAddress: text('from_address'),
    to: jsonb('to').$type<Address[]>().notNull().default([]),
    cc: jsonb('cc').$type<Address[]>().notNull().default([]),
    date: timestamp('date', { withTimezone: true }).notNull(),
    snippet: text('snippet').notNull().default(''),
    flags: jsonb('flags').$type<string[]>().notNull().default([]),
    seen: boolean('seen').notNull().default(false),
    flagged: boolean('flagged').notNull().default(false),
    hasAttachments: boolean('has_attachments').notNull().default(false),
    size: integer('size').notNull().default(0),
    search: tsvector('search').generatedAlwaysAs(
      sql`to_tsvector('spanish', coalesce(subject, '') || ' ' || coalesce(from_name, '') || ' ' || coalesce(from_address, '') || ' ' || coalesce(snippet, ''))`,
    ),
    createdAt: timestamp('created_at', { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [
    uniqueIndex('messages_folder_uid_uq').on(t.folderId, t.uid),
    index('messages_account_date_idx').on(t.accountId, t.date),
    index('messages_folder_date_idx').on(t.folderId, t.date),
    index('messages_search_idx').using('gin', t.search),
  ],
);

export const pushSubscriptions = pgTable('push_subscriptions', {
  id: uuid('id').primaryKey().defaultRandom(),
  userId: uuid('user_id')
    .notNull()
    .references(() => users.id, { onDelete: 'cascade' }),
  endpoint: text('endpoint').notNull().unique(),
  keys: jsonb('keys').$type<{ p256dh: string; auth: string }>().notNull(),
  userAgent: text('user_agent'),
  createdAt: timestamp('created_at', { withTimezone: true }).notNull().defaultNow(),
});

export type User = typeof users.$inferSelect;
export type MailAccount = typeof mailAccounts.$inferSelect;
export type Folder = typeof folders.$inferSelect;
export type Message = typeof messages.$inferSelect;
