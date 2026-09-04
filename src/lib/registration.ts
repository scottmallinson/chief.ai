import { invoke } from '@tauri-apps/api/core';

/** Where the client id a service signs in with came from. */
export type RegistrationSource = 'environment' | 'stored' | 'builtIn' | 'missing';

/**
 * One service's OAuth registration.
 *
 * The client id is here in the clear on purpose. It is public by design —
 * GitHub's device flow has no secret and an Entra public client has none
 * either — and the user cannot tell which registration they are signing in
 * against unless Chief says which one it is using.
 */
export interface Registration {
  /** The value stored in `integration_accounts.service`. */
  service: string;
  /** The id that would be used, or null when there is none anywhere. */
  clientId: string | null;
  source: RegistrationSource;
  /** Whether this build carries a default to fall back to. */
  hasBuiltIn: boolean;
  /** Whether an environment variable is overriding whatever is stored. */
  overriddenByEnvironment: boolean;
}

/** Which registration each service Chief signs into would use. */
export function signInRegistrations(): Promise<Registration[]> {
  return invoke<Registration[]>('sign_in_registrations');
}

/** Sign this service in against a registration of the user's own. */
export function setSignInRegistration(service: string, clientId: string): Promise<Registration[]> {
  return invoke<Registration[]>('set_sign_in_registration', { service, clientId });
}

/** Go back to whatever the build carries. */
export function clearSignInRegistration(service: string): Promise<Registration[]> {
  return invoke<Registration[]>('clear_sign_in_registration', { service });
}
