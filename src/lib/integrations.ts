import { invoke } from '@tauri-apps/api/core';

/** Whether a service is connected, as the backend reports it. */
export interface Connection {
  service: string;
  connected: boolean;
  /** ISO-8601, or null when not connected. */
  connectedAt: string | null;
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

/** Whether GitHub is connected. */
export function githubConnection(): Promise<Connection> {
  return invoke<Connection>('github_connection');
}

/** Begin signing in and get the code the user must enter. */
export function startGithubLogin(): Promise<DeviceLogin> {
  return invoke<DeviceLogin>('start_github_login');
}

/** Wait for the user to finish in the browser, then store the token. */
export function finishGithubLogin(): Promise<Connection> {
  return invoke<Connection>('finish_github_login');
}

/** Forget the stored GitHub credential. */
export function disconnectGithub(): Promise<Connection> {
  return invoke<Connection>('disconnect_github');
}
