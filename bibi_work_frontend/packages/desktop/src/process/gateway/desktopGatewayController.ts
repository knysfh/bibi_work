type StoppableWorker = {
  stop: () => void;
};

type GatewayServer = {
  port: number;
  stop: () => Promise<void>;
};

type OidcLoopbackServer = {
  port: number;
  stop: () => Promise<void>;
};

export type DesktopGatewayControllerOptions = {
  startServer: (backendPort: number) => Promise<GatewayServer>;
  startOidcLoopback: () => Promise<OidcLoopbackServer>;
  startLocalMcpWorker: (backendPort: number) => StoppableWorker;
  startBrowserWorker: (backendPort: number) => StoppableWorker;
  startDesktopAcpWorker: (backendPort: number) => StoppableWorker;
  onLoopbackStartError?: (error: unknown) => void;
  onStarted?: (gatewayPort: number, backendPort: number) => void;
};

export class DesktopGatewayController {
  private server: GatewayServer | null = null;
  private oidcLoopback: OidcLoopbackServer | null = null;
  private localMcpWorker: StoppableWorker | null = null;
  private browserWorker: StoppableWorker | null = null;
  private desktopAcpWorker: StoppableWorker | null = null;
  private startPromise: Promise<number> | null = null;
  private stopPromise: Promise<void> | null = null;

  constructor(private readonly options: DesktopGatewayControllerOptions) {}

  get port(): number | null {
    return this.server?.port ?? null;
  }

  get oidcLoopbackPort(): number | null {
    return this.oidcLoopback?.port ?? null;
  }

  async ensureStarted(backendPort: number): Promise<number> {
    if (this.stopPromise) await this.stopPromise;
    if (this.server) return this.server.port;
    this.startPromise ??= this.start(backendPort).catch((error) => {
      this.startPromise = null;
      throw error;
    });
    return this.startPromise;
  }

  async stop(): Promise<void> {
    this.stopPromise ??= this.stopResources().finally(() => {
      this.stopPromise = null;
    });
    return this.stopPromise;
  }

  private async start(backendPort: number): Promise<number> {
    const server = await this.options.startServer(backendPort);
    this.server = server;
    try {
      if (!this.oidcLoopback) {
        try {
          this.oidcLoopback = await this.options.startOidcLoopback();
        } catch (error) {
          this.options.onLoopbackStartError?.(error);
        }
      }

      this.localMcpWorker ??= this.options.startLocalMcpWorker(backendPort);
      this.browserWorker ??= this.options.startBrowserWorker(backendPort);
      this.desktopAcpWorker ??= this.options.startDesktopAcpWorker(backendPort);
      this.options.onStarted?.(server.port, backendPort);
      return server.port;
    } catch (error) {
      await this.cleanupOwnedResources();
      throw error;
    }
  }

  private async stopResources(): Promise<void> {
    const pendingStart = this.startPromise;
    if (pendingStart) await pendingStart.catch((): undefined => undefined);

    this.startPromise = null;
    await this.cleanupOwnedResources();
  }

  private async cleanupOwnedResources(): Promise<void> {
    this.localMcpWorker?.stop();
    this.localMcpWorker = null;
    this.browserWorker?.stop();
    this.browserWorker = null;
    this.desktopAcpWorker?.stop();
    this.desktopAcpWorker = null;

    const server = this.server;
    this.server = null;
    if (server) await server.stop();

    const oidcLoopback = this.oidcLoopback;
    this.oidcLoopback = null;
    if (oidcLoopback) await oidcLoopback.stop();
  }
}
