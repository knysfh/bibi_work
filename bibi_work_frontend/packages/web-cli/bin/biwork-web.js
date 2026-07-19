#!/usr/bin/env node
import('../src/index.js').catch((err) => {
  console.error('Failed to start biwork-web:', err);
  process.exit(1);
});
