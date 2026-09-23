import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Whether closing the window keeps Chief running, as `window_behaviour` returns it. */
export interface WindowBehaviour {
  /** Whether this machine has a tray to keep running in. Without one, closing quits. */
  tray: boolean;
  /** The stored choice, or null if the user has not been asked yet. */
  keepRunning: boolean | null;
  /** What this platform calls the tray: `menu bar` or `system tray`. */
  trayName: string;
}

export function windowBehaviour(): Promise<WindowBehaviour> {
  return invoke<WindowBehaviour>('window_behaviour');
}

export function setKeepRunning(keepRunning: boolean): Promise<void> {
  return invoke<void>('set_keep_running', { keepRunning });
}

/** Finish the close the question interrupted, according to the stored choice. */
export function closeWindow(): Promise<void> {
  return invoke<void>('close_window');
}

/**
 * Called when the window is closed for the first time with no choice stored.
 * Returns a function that stops listening.
 */
export function onCloseQuestion(handler: () => void) {
  return listen<null>('close-to-tray-question', () => handler());
}

/** Whether Chief is set to open at login, as the operating system has it. */
export function launchAtLogin(): Promise<boolean> {
  return invoke<boolean>('launch_at_login');
}

/** Add Chief to the login items, or take it out. */
export function setLaunchAtLogin(enabled: boolean): Promise<void> {
  return invoke<void>('set_launch_at_login', { enabled });
}
