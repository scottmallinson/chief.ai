import { invoke } from '@tauri-apps/api/core';

/** The services Chief knows how to connect. */
export const GITHUB = 'github';

/** Outlook mail and calendar, through Microsoft Graph. */
export const MICROSOFT = 'microsoft';

/** A calendar subscribed to by URL rather than signed in to. */
export const CALENDAR = 'calendar';

/** Linear, read with a personal API key rather than an OAuth grant. */
export const LINEAR = 'linear';

/** Jira and Confluence, through Atlassian's Remote MCP server. */
export const ATLASSIAN = 'atlassian';

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
  /** Where they type it, if they have to. */
  verificationUri: string;
  /**
   * The same page with the code already in the box, which is what Chief
   * opens.
   *
   * Built by Rust rather than sent by GitHub — the prefill works but is not
   * documented, so `verificationUri` and `userCode` stay beside it and stay
   * on screen. A prefill that stops working costs a keystroke, not the
   * sign-in.
   */
  verificationUriComplete: string;
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

/**
 * What a finished sign-in did.
 *
 * The account list alone cannot say. Signing in as somebody already connected
 * replaces that account's credential rather than adding a row, so the list
 * comes back exactly as long as it went out — which is indistinguishable, on
 * screen, from the sign-in having done nothing at all.
 */
export interface Connected {
  /** Every account, whatever the service. */
  accounts: Account[];
  /** The account this sign-in landed on. */
  account: Account;
  /** Whether it was already connected, so nothing was added. */
  reconnected: boolean;
}

/** Wait for the user to finish signing in, then store the credential. */
export function finishLogin(service: string): Promise<Connected> {
  return invoke<Connected>('finish_login', { service });
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
 * Connect Jira and Confluence with an Atlassian API token.
 *
 * The other way in, for organisations whose administrator has switched off the
 * Rovo MCP server. Atlassian's Basic auth takes three things — the site, the
 * email the token belongs to, and the token — and the token is a bearer
 * credential, so it goes straight to Rust and is never returned or displayed.
 *
 * The site may be typed as a bare name: "acme" becomes
 * "https://acme.atlassian.net".
 */
export function addAtlassianToken(site: string, email: string, token: string): Promise<Account> {
  return invoke<Account>('add_atlassian_token', { site, email, token });
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
