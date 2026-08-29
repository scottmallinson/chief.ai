import { NotebookText, Settings, Sunrise, type LucideIcon } from 'lucide-react';

/**
 * The top-level destinations of the app shell.
 *
 * Chat is deliberately not among them. It is an overlay drawer reachable from
 * every destination, because a question about your work is something you ask
 * *while* looking at it, not somewhere you navigate to instead.
 */
export const VIEWS = ['today', 'work-log', 'settings'] as const;

export type View = (typeof VIEWS)[number];

export interface NavItem {
  id: View;
  label: string;
  description: string;
  icon: LucideIcon;
}

export const NAV_ITEMS: readonly NavItem[] = [
  {
    id: 'today',
    label: 'Today',
    description: 'Your brief, and what you shipped lately',
    icon: Sunrise,
  },
  {
    id: 'work-log',
    label: 'Work Log',
    description: 'A chronological record of what you shipped',
    icon: NotebookText,
  },
  {
    id: 'settings',
    label: 'Settings',
    description: 'Model, integrations and local data',
    icon: Settings,
  },
];
