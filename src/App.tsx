import { useEffect, useState } from 'react';
import { Analytics } from '@vercel/analytics/react';

import { BriefList } from '@/components/BriefList';
import { ChiefMark } from '@/components/ChiefMark';
import { DraftEditor } from '@/components/DraftEditor';
import { Drawer } from '@/components/Drawer';
import { Layout } from '@/components/Layout';
import { ChatView } from '@/components/views/ChatView';
import { SettingsView } from '@/components/views/SettingsView';
import { SetupView } from '@/components/views/SetupView';
import { TodayView } from '@/components/views/TodayView';
import { WorkLogView } from '@/components/views/WorkLogView';
import { useBrief } from '@/hooks/use-brief';
import { useChat } from '@/hooks/use-chat';
import { useProposals } from '@/hooks/use-proposals';
import { useIntegrations } from '@/hooks/use-integrations';
import type { Proposal } from '@/lib/proposals';
import { checkReadiness, isReady, type Readiness } from '@/lib/setup';
import type { View } from '@/lib/navigation';

function App() {
  const [activeView, setActiveView] = useState<View>('today');
  const [chatOpen, setChatOpen] = useState(false);
  // The draft the drawer is showing instead of chat, when there is one.
  const [editing, setEditing] = useState<Proposal | null>(null);
  // Null until we know whether this machine can answer anything yet.
  const [ready, setReady] = useState<boolean | null>(null);
  const [readiness, setReadiness] = useState<Readiness | null>(null);

  // Owned here rather than inside `TodayView`, because the list pane sits
  // beside that view in the shell rather than inside it, and the two have to
  // be looking at the same day.
  const brief = useBrief();
  // Owned here so it survives the drawer closing. See `ChatView`.
  const chat = useChat();
  const proposals = useProposals();
  // The header needs to know how many accounts exist to tell "nothing is
  // connected" from "something is connected and has never been read".
  const { accounts } = useIntegrations();

  useEffect(() => {
    let cancelled = false;

    checkReadiness()
      .then((current) => {
        if (cancelled) return;

        setReadiness(current);
        setReady(isReady(current));
      })
      .catch(() => {
        // If the check itself fails, the setup screen explains why.
        if (!cancelled) setReady(false);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  if (ready === null) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4">
        <ChiefMark size={40} breathing />
        <p className="micro text-muted-foreground" role="status">
          Starting Chief
        </p>
      </div>
    );
  }

  if (!ready) {
    return <SetupView onSkip={() => setReady(true)} />;
  }

  return (
    <>
      <Layout
        activeView={activeView}
        onNavigate={setActiveView}
        onOpenChat={() => setChatOpen(true)}
        model={readiness?.model}
        accounts={accounts.length}
        list={
          activeView === 'today' ? (
            <BriefList
              days={brief.days}
              selected={brief.selected}
              today={brief.today}
              onSelect={brief.select}
            />
          ) : undefined
        }
      >
        {activeView === 'today' && (
          <TodayView
            brief={brief.brief}
            day={brief.selected}
            today={brief.today}
            status={brief.status}
            error={brief.error}
            onWrite={brief.write}
            onShowToday={() => brief.select(brief.today)}
            proposals={proposals.proposals}
            onEditProposal={(proposal) => {
              setEditing(proposal);
              setChatOpen(true);
            }}
            onProposalDismissed={proposals.forget}
          />
        )}
        {activeView === 'work-log' && <WorkLogView />}
        {activeView === 'settings' && <SettingsView />}
      </Layout>

      {/*
        Mounted outside the shell so it overlays rather than pushes: the detail
        column behind it is exactly as wide open as closed, which is the whole
        reason chat moved here from a destination of its own.
      */}
      <Drawer
        open={chatOpen}
        title={editing === null ? 'Ask Chief' : 'Edit draft'}
        onClose={() => {
          setChatOpen(false);
          setEditing(null);
        }}
      >
        {editing === null ? (
          <ChatView chat={chat} />
        ) : (
          <DraftEditor proposal={editing} onSaved={proposals.reload} />
        )}
      </Drawer>
      <Analytics />
    </>
  );
}

export default App;
