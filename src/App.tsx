import { useEffect, useState } from 'react';

import { Layout } from '@/components/Layout';
import { ChatView } from '@/components/views/ChatView';
import { SettingsView } from '@/components/views/SettingsView';
import { SetupView } from '@/components/views/SetupView';
import { WorkLogView } from '@/components/views/WorkLogView';
import { checkReadiness, isReady } from '@/lib/setup';
import type { View } from '@/lib/navigation';

function App() {
  const [activeView, setActiveView] = useState<View>('chat');
  // Null until we know whether this machine can answer anything yet.
  const [ready, setReady] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;

    checkReadiness()
      .then((readiness) => {
        if (!cancelled) setReady(isReady(readiness));
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
      <div className="flex h-full items-center justify-center">
        <p className="text-sm text-muted-foreground" role="status">
          Starting Chief…
        </p>
      </div>
    );
  }

  if (!ready) {
    return <SetupView onSkip={() => setReady(true)} />;
  }

  return (
    <Layout activeView={activeView} onNavigate={setActiveView}>
      {activeView === 'chat' && <ChatView />}
      {activeView === 'work-log' && <WorkLogView />}
      {activeView === 'settings' && <SettingsView />}
    </Layout>
  );
}

export default App;
