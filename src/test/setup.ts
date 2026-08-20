import '@testing-library/jest-dom/vitest';

import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// jsdom has no layout, so it does not implement scrolling. Views that follow a
// growing answer call this on every update; without a stand-in they throw.
Element.prototype.scrollIntoView = () => undefined;

afterEach(() => {
  cleanup();
});
