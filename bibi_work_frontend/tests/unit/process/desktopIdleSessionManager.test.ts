import { afterEach, describe, expect, it, vi } from 'vitest';
import { DesktopIdleSessionManager } from '@process/auth/desktopIdleSessionManager';

describe('DesktopIdleSessionManager', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('expires an active session after the configured inactivity period', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const onIdleTimeout = vi.fn();
    const manager = new DesktopIdleSessionManager({ idleTimeoutMs: 1_000, onIdleTimeout });

    manager.startSession();
    await vi.advanceTimersByTimeAsync(999);
    expect(onIdleTimeout).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1);
    expect(onIdleTimeout).toHaveBeenCalledOnce();
  });

  it('resets the deadline when meaningful activity is recorded', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const onIdleTimeout = vi.fn();
    const manager = new DesktopIdleSessionManager({ idleTimeoutMs: 1_000, onIdleTimeout });

    manager.startSession();
    await vi.advanceTimersByTimeAsync(900);
    expect(manager.recordActivity()).toBe(true);

    await vi.advanceTimersByTimeAsync(999);
    expect(onIdleTimeout).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(onIdleTimeout).toHaveBeenCalledOnce();
  });

  it('does not revive a session when delayed activity arrives after the deadline', async () => {
    let now = 1_000;
    const onIdleTimeout = vi.fn();
    const manager = new DesktopIdleSessionManager({
      idleTimeoutMs: 1_000,
      now: () => now,
      onIdleTimeout,
      setTimeoutImpl: vi.fn(() => 1 as unknown as ReturnType<typeof setTimeout>),
      clearTimeoutImpl: vi.fn(),
    });

    manager.startSession();
    now = 2_001;

    expect(manager.recordActivity()).toBe(false);
    await Promise.resolve();
    expect(onIdleTimeout).toHaveBeenCalledOnce();
  });

  it('evaluates elapsed wall time after system resume', async () => {
    let now = 1_000;
    const onIdleTimeout = vi.fn();
    const manager = new DesktopIdleSessionManager({
      idleTimeoutMs: 1_000,
      now: () => now,
      onIdleTimeout,
      setTimeoutImpl: vi.fn(() => 1 as unknown as ReturnType<typeof setTimeout>),
      clearTimeoutImpl: vi.fn(),
    });

    manager.startSession();
    now = 3_000;
    manager.evaluate();
    await Promise.resolve();

    expect(onIdleTimeout).toHaveBeenCalledOnce();
  });
});
