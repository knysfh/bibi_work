# @biwork/web-host

WebUI host package for BiWork - zero Electron dependency.

## Responsibilities

- Serve the desktop renderer as a web application.
- Reverse proxy `/api` and `/ws` to an already running `bibi_work_backend`.
- Keep the web host independent from Electron and backend process management.

## Usage

```ts
import { startWebHost } from '@biwork/web-host';

const handle = await startWebHost({
  staticDir: '/path/to/out/renderer',
  backend: {
    kind: 'useExistingBackend',
    port: 8361,
  },
});

console.log(`WebUI running at ${handle.url}`);

await handle.stop();
```

The host never downloads, starts, stops, migrates, or repairs the backend.
