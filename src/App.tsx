import { useEffect, useState } from 'react';

import { ChiefMark } from '@/components/ChiefMark';
import { Layout } from '@/components/Layout';
import { ChatView } from '@/components/views/ChatView';
import { SettingsView } from '@/components/views/SettingsView';
import { SetupView } from '@/components/views/SetupView';
import { WorkLogView } from '@/components/views/WorkLogView';
import { checkReadiness, isReady, type Readiness } from '@/lib/setup';
import type { View } from '@/lib/navigation';

function App() {
  const [activeView, setActiveView] = useState<View>('chat');
  // Null until we know whether this machine can answer anything yet.
  const [ready, setReady] = useState<boolean | null>(null);
  const [readiness, setReadiness] = useState<Readiness | null>(null);

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
    <Layout activeView={activeView} onNavigate={setActiveView} model={readiness?.model}>
      {activeView === 'chat' && <ChatView />}
      {activeView === 'work-log' && <WorkLogView />}
      {activeView === 'settings' && <SettingsView />}
    </Layout>
  );
}

export default App;
