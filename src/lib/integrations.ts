import { invoke } from '@tauri-apps/api/core';

/** The services Chief knows how to connect. */
export const GITHUB = 'github';

/** Outlook mail and calendar, through Microsoft Graph. */
export const MICROSOFT = 'microsoft';

/** A calendar subscribed to by URL rather than signed in to. */
export const CALENDAR = 'calendar';

/** Linear, read with a personal API key rather than an OAuth grant. */
export const LINEAR = 'linear';

/** One connected account, as the backend reports it. Carries no secret. */
export interface Account {
  id: number;
  service: string;
  /** The provider's stable identifier for this account. */
  accountKey: string;
  /** What the user called it, if they named it. */
  label: string | null;
  /** What to show when there is no label: a login, an address, a site. */
  identity: string | null;
  /** ISO-8601. */
  connectedAt: string;
}

/** What the user must do in the browser to finish signing in. */
export interface DeviceLogin {
  kind: 'device';
  /** The code they type into the provider. */
  userCode: string;
  /** Where they type it. */
  verificationUri: string;
  /** Seconds until the code stops working. */
  expiresIn: number;
}

/**
 * A sign-in that happens in the browser and comes back to a port Chief holds.
 *
 * There is no code to show and nothing to copy: the whole exchange is the
 * browser going somewhere and returning.
 */
export interface BrowserLogin {
  kind: 'browser';
  /** Where to send them. */
  url: string;
}

/**
 * What the user has to do next.
 *
 * Two providers, two shapes, and neither is a special case of the other:
 * GitHub shows a code to type somewhere else, Microsoft opens a browser and
 * waits. Tagged, so a renderer that forgets a case fails to compile.
 */
export type Login = DeviceLogin | BrowserLogin;

/** Every connected account, whatever the service. */
export function connections(): Promise<Account[]> {
  return invoke<Account[]>('connections');
}

/** Begin signing in and get what the user has to do next. */
export function startLogin(service: string): Promise<Login> {
  return invoke<Login>('start_login', { service });
}

/** Wait for the user to finish signing in, then store the credential. */
export function finishLogin(service: string): Promise<Account[]> {
  return invoke<Account[]>('finish_login', { service });
}

/** Forget one account's credential. */
/**
 * Subscribe to a calendar by its published address.
 *
 * The address is a bearer credential — anyone holding it can read the whole
 * calendar — so it goes straight to Rust and is never returned or displayed.
 */
export function addCalendar(url: string, label: string | null): Promise<Account> {
  return invoke<Account>('add_calendar', { url, label });
}

/**
 * Connect Linear with a personal API key.
 *
 * The key is a bearer credential carrying that person's whole Linear access, so
 * it goes straight to Rust and is never returned or displayed.
 */
export function addLinearKey(key: string): Promise<Account> {
  return invoke<Account>('add_linear_key', { key });
}

/**
 * What unlinking an account would take with it.
 *
 * Read before the confirmation is shown, because a person unlinking an account
 * is thinking about a credential and has no reason to know that months of
 * their work log and a pile of drafts are behind it.
 */
export interface AccountData {
  entries: number;
  proposals: number;
}

export function accountData(accountId: number): Promise<AccountData> {
  return invoke<AccountData>('account_data', { accountId });
}

/**
 * Forget one account: its credential, and everything it put on this machine.
 *
 * **Irreversible, and it removes more than the word suggests** — which is why
 * the interface confirms with the counts from {@link accountData} first. It
 * used to delete the credential row alone, leaving the account's work log
 * entries, its drafts and their markdown behind.
 */
export function disconnect(accountId: number): Promise<Account[]> {
  return invoke<Account[]>('disconnect', { accountId });
}

/** Name an account, or clear its name. */
export function labelAccount(accountId: number, label: string | null): Promise<Account[]> {
  return invoke<Account[]>('label_account', { accountId, label });
}

/** What to call an account on screen. */
export function accountName(account: Account): string {
  return account.label ?? account.identity ?? account.accountKey;
}
