import { invoke } from '@tauri-apps/api/core';

/** The name the backend knows GitHub by. Services are values, not commands. */
export const GITHUB = 'github';

/**
 * One connected account, as the backend reports it.
 *
 * This is the whole of what the renderer is told about a connection: the
 * credential itself never leaves Rust, so there is nothing secret here to
 * leak into a devtools console or a React state dump.
 */
export interface Account {
  id: number;
  service: string;
  /** The provider's stable identifier for this account. */
  accountKey: string;
  /** What the user called it, if they named it. */
  label: string | null;
  /** What to show when there is no label: a login, an address, a site. */
  identity: string | null;
  /** When the credential was stored, ISO-8601. */
  connectedAt: string;
}

/** What the user must do in the browser to finish signing in. */
export interface DeviceLogin {
  /** The code they type into GitHub. */
  userCode: string;
  /** Where they type it. */
  verificationUri: string;
  /** Seconds until the code stops working. */
  expiresIn: number;
}

/**
 * Every connected account, whatever the service.
 *
 * The backend answers with the whole list rather than one service's worth, so
 * a screen that grows a second provider reads the same command.
 */
export function connections(): Promise<Account[]> {
  return invoke<Account[]>('connections');
}

/** Begin signing in to a service and get the code the user must enter. */
export function startLogin(service: string): Promise<DeviceLogin> {
  return invoke<DeviceLogin>('start_login', { service });
}

/**
 * Wait for the user to finish in the browser, then store the credential.
 *
 * Answers with the accounts as they now stand, so the caller never has to
 * re-read them to find out what it just connected.
 */
export function finishLogin(service: string): Promise<Account[]> {
  return invoke<Account[]>('finish_login', { service });
}

/** Forget one account's credential, and report what is left. */
export function disconnectAccount(accountId: number): Promise<Account[]> {
  return invoke<Account[]>('disconnect', { accountId });
}
