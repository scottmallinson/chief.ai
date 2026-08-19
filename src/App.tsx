import { useState } from 'react';

import { Layout } from '@/components/Layout';
import { ChatView } from '@/components/views/ChatView';
import { SettingsView } from '@/components/views/SettingsView';
import { WorkLogView } from '@/components/views/WorkLogView';
import type { View } from '@/lib/navigation';

function App() {
  const [activeView, setActiveView] = useState<View>('chat');

  return (
    <Layout activeView={activeView} onNavigate={setActiveView}>
      {activeView === 'chat' && <ChatView />}
      {activeView === 'work-log' && <WorkLogView />}
      {activeView === 'settings' && <SettingsView />}
    </Layout>
  );
}

export default App;
