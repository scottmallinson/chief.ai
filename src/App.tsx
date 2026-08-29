import { useEffect, useState } from 'react';

import { BriefList } from '@/components/BriefList';
import { ChiefMark } from '@/components/ChiefMark';
import { Drawer } from '@/components/Drawer';
import { Layout } from '@/components/Layout';
import { ChatView } from '@/components/views/ChatView';
import { SettingsView } from '@/components/views/SettingsView';
import { SetupView } from '@/components/views/SetupView';
import { TodayView } from '@/components/views/TodayView';
import { WorkLogView } from '@/components/views/WorkLogView';
import { useBrief } from '@/hooks/use-brief';
import { useChat } from '@/hooks/use-chat';
import { checkReadiness, isReady, type Readiness } from '@/lib/setup';
import type { View } from '@/lib/navigation';

function App() {
  const [activeView, setActiveView] = useState<View>('today');
  const [chatOpen, setChatOpen] = useState(false);
  // Null until we know whether this machine can answer anything yet.
  const [ready, setReady] = useState<boolean | null>(null);
  const [readiness, setReadiness] = useState<Readiness | null>(null);

  // Owned here rather than inside `TodayView`, because the list pane sits
  // beside that view in the shell rather than inside it, and the two have to
  // be looking at the same day.
  const brief = useBrief();
  // Owned here so it survives the drawer closing. See `ChatView`.
  const chat = useChat();

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
        list={
          activeView === 'today' ? (
            <BriefList
              days={brief.days}
              selected={brief.brief?.date ?? null}
              onSelect={brief.select}
            />
          ) : undefined
        }
      >
        {activeView === 'today' && (
          <TodayView
            brief={brief.brief}
            status={brief.status}
            error={brief.error}
            onWrite={brief.write}
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
      <Drawer open={chatOpen} title="Ask Chief" onClose={() => setChatOpen(false)}>
        <ChatView chat={chat} />
      </Drawer>
    </>
  );
}

export default App;
