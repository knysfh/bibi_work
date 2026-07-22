const DEFAULT_IDLE_TIMEOUT_MS = 30 * 60 * 1000;

type TimerHandle = ReturnType<typeof setTimeout>;

export type DesktopIdleSessionManagerOptions = {
  clearTimeoutImpl?: typeof clearTimeout;
  idleTimeoutMs?: number;
  now?: () => number;
  onIdleTimeout: () => void | Promise<void>;
  setTimeoutImpl?: typeof setTimeout;
};

export class DesktopIdleSessionManager {
  private active = false;
  private expiring = false;
  private lastActivityAt: number | null = null;
  private timer: TimerHandle | null = null;

  constructor(private readonly options: DesktopIdleSessionManagerOptions) {}

  startSession(): void {
    this.active = true;
    this.expiring = false;
    this.lastActivityAt = this.now();
    this.scheduleEvaluation();
  }

  stopSession(): void {
    this.active = false;
    this.expiring = false;
    this.lastActivityAt = null;
    this.clearTimer();
  }

  recordActivity(): boolean {
    if (!this.active || this.expiring || this.lastActivityAt === null) return false;
    const now = this.now();
    if (now - this.lastActivityAt >= this.idleTimeoutMs()) {
      this.expireIfIdle();
      return false;
    }
    this.lastActivityAt = now;
    this.scheduleEvaluation();
    return true;
  }

  evaluate(): void {
    if (!this.active || this.expiring || this.lastActivityAt === null) return;
    if (this.now() - this.lastActivityAt >= this.idleTimeoutMs()) {
      this.expireIfIdle();
      return;
    }
    this.scheduleEvaluation();
  }

  private now(): number {
    return (this.options.now ?? Date.now)();
  }

  private idleTimeoutMs(): number {
    return Math.max(1, this.options.idleTimeoutMs ?? DEFAULT_IDLE_TIMEOUT_MS);
  }

  private scheduleEvaluation(): void {
    this.clearTimer();
    if (!this.active || this.expiring || this.lastActivityAt === null) return;
    const delay = Math.max(1, this.lastActivityAt + this.idleTimeoutMs() - this.now());
    this.timer = (this.options.setTimeoutImpl ?? setTimeout)(() => {
      this.timer = null;
      this.evaluate();
    }, delay);
  }

  private expireIfIdle(): void {
    if (!this.active || this.expiring) return;
    this.expiring = true;
    this.clearTimer();
    void Promise.resolve(this.options.onIdleTimeout())
      .catch(() => undefined)
      .finally(() => {
        this.active = false;
        this.expiring = false;
        this.lastActivityAt = null;
      });
  }

  private clearTimer(): void {
    if (!this.timer) return;
    (this.options.clearTimeoutImpl ?? clearTimeout)(this.timer);
    this.timer = null;
  }
}
