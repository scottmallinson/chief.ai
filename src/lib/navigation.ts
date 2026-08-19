import { MessageSquare, NotebookText, Settings, type LucideIcon } from 'lucide-react';

/** The top-level destinations of the app shell. */
export const VIEWS = ['chat', 'work-log', 'settings'] as const;

export type View = (typeof VIEWS)[number];

export interface NavItem {
  id: View;
  label: string;
  description: string;
  icon: LucideIcon;
}

export const NAV_ITEMS: readonly NavItem[] = [
  {
    id: 'chat',
    label: 'Chat',
    description: 'Ask your chief of staff about your work',
    icon: MessageSquare,
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
