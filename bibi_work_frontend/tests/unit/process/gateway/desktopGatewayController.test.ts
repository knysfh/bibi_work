import { describe, expect, it, vi } from 'vitest';
import { DesktopGatewayController } from '@process/gateway/desktopGatewayController';

describe('DesktopGatewayController', () => {
  it('coalesces concurrent starts and owns gateway resources', async () => {
    const stopServer = vi.fn(async () => undefined);
    const stopLoopback = vi.fn(async () => undefined);
    const stopMcp = vi.fn();
    const stopBrowser = vi.fn();
    const stopAcp = vi.fn();
    const startServer = vi.fn(async () => ({ port: 43120, stop: stopServer }));
    const startOidcLoopback = vi.fn(async () => ({ port: 43121, stop: stopLoopback }));
    const startLocalMcpWorker = vi.fn(() => ({ stop: stopMcp }));
    const startBrowserWorker = vi.fn(() => ({ stop: stopBrowser }));
    const startDesktopAcpWorker = vi.fn(() => ({ stop: stopAcp }));
    const onStarted = vi.fn();
    const controller = new DesktopGatewayController({
      startServer,
      startOidcLoopback,
      startLocalMcpWorker,
      startBrowserWorker,
      startDesktopAcpWorker,
      onStarted,
    });

    await expect(Promise.all([controller.ensureStarted(8361), controller.ensureStarted(8361)])).resolves.toEqual([
      43120, 43120,
    ]);
    expect(startServer).toHaveBeenCalledOnce();
    expect(startOidcLoopback).toHaveBeenCalledOnce();
    expect(startLocalMcpWorker).toHaveBeenCalledOnce();
    expect(startBrowserWorker).toHaveBeenCalledOnce();
    expect(startDesktopAcpWorker).toHaveBeenCalledOnce();
    expect(onStarted).toHaveBeenCalledWith(43120, 8361);
    expect(controller.port).toBe(43120);
    expect(controller.oidcLoopbackPort).toBe(43121);

    await controller.stop();
    expect(stopMcp).toHaveBeenCalledOnce();
    expect(stopBrowser).toHaveBeenCalledOnce();
    expect(stopAcp).toHaveBeenCalledOnce();
    expect(stopServer).toHaveBeenCalledOnce();
    expect(stopLoopback).toHaveBeenCalledOnce();
    expect(controller.port).toBeNull();
    expect(controller.oidcLoopbackPort).toBeNull();
  });

  it('can restart cleanly after stop', async () => {
    const stopServers: Array<ReturnType<typeof vi.fn>> = [];
    const controller = new DesktopGatewayController({
      startServer: vi.fn(async () => {
        const stop = vi.fn(async () => undefined);
        stopServers.push(stop);
        return { port: 44000 + stopServers.length, stop };
      }),
      startOidcLoopback: vi.fn(async () => ({ port: 45000, stop: vi.fn(async () => undefined) })),
      startLocalMcpWorker: vi.fn(() => ({ stop: vi.fn() })),
      startBrowserWorker: vi.fn(() => ({ stop: vi.fn() })),
      startDesktopAcpWorker: vi.fn(() => ({ stop: vi.fn() })),
    });

    await expect(controller.ensureStarted(8361)).resolves.toBe(44001);
    await controller.stop();
    await expect(controller.ensureStarted(8361)).resolves.toBe(44002);
    expect(stopServers[0]).toHaveBeenCalledOnce();
    await controller.stop();
    expect(stopServers[1]).toHaveBeenCalledOnce();
  });

  it('keeps the gateway available when the optional OIDC loopback fails', async () => {
    const error = new Error('port unavailable');
    const onLoopbackStartError = vi.fn();
    const controller = new DesktopGatewayController({
      startServer: vi.fn(async () => ({ port: 46000, stop: vi.fn(async () => undefined) })),
      startOidcLoopback: vi.fn(async () => Promise.reject(error)),
      startLocalMcpWorker: vi.fn(() => ({ stop: vi.fn() })),
      startBrowserWorker: vi.fn(() => ({ stop: vi.fn() })),
      startDesktopAcpWorker: vi.fn(() => ({ stop: vi.fn() })),
      onLoopbackStartError,
    });

    await expect(controller.ensureStarted(8361)).resolves.toBe(46000);
    expect(onLoopbackStartError).toHaveBeenCalledWith(error);
    expect(controller.oidcLoopbackPort).toBeNull();
    await controller.stop();
  });

  it('rolls back partially started resources when a mandatory worker fails', async () => {
    const stopServer = vi.fn(async () => undefined);
    const stopLoopback = vi.fn(async () => undefined);
    const stopMcp = vi.fn();
    const controller = new DesktopGatewayController({
      startServer: vi.fn(async () => ({ port: 47000, stop: stopServer })),
      startOidcLoopback: vi.fn(async () => ({ port: 47001, stop: stopLoopback })),
      startLocalMcpWorker: vi.fn(() => ({ stop: stopMcp })),
      startBrowserWorker: vi.fn(() => ({ stop: vi.fn() })),
      startDesktopAcpWorker: vi.fn(() => {
        throw new Error('ACP worker failed');
      }),
    });

    await expect(controller.ensureStarted(8361)).rejects.toThrow('ACP worker failed');
    expect(stopMcp).toHaveBeenCalledOnce();
    expect(stopServer).toHaveBeenCalledOnce();
    expect(stopLoopback).toHaveBeenCalledOnce();
    expect(controller.port).toBeNull();
    expect(controller.oidcLoopbackPort).toBeNull();
  });
});
