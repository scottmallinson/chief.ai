import React from 'react';
import ReactDOM from 'react-dom/client';

import App from '@/App';
import { CloseQuestion } from '@/components/CloseQuestion';
import '@/styles/globals.css';

const rootElement = document.getElementById('root');

if (!rootElement) {
  throw new Error('Root element #root was not found in index.html');
}

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <App />
    {/* Beside the app rather than inside it, so it can be asked whatever the
        app is showing — the setup screen included. */}
    <CloseQuestion />
  </React.StrictMode>,
);
