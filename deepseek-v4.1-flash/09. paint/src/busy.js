/**
 * Busy state guard.
 *
 * Long operations (adjustment previews, large transforms, encoding) run with the
 * UI in a busy state: controls are disabled, the status bar shows progress, and
 * pointer input on the canvas is ignored so two operations can never interleave
 * and corrupt the document or the history.
 */
export class BusyState {
  constructor() {
    this.count = 0;
    this.label = '';
    this.listeners = new Set();
  }

  get busy() { return this.count > 0; }

  /** @param {(state:{busy:boolean,label:string})=>void} listener */
  subscribe(listener) {
    this.listeners.add(listener);
    listener({ busy: this.busy, label: this.label });
    return () => this.listeners.delete(listener);
  }

  notify() {
    for (const listener of this.listeners) listener({ busy: this.busy, label: this.label });
  }

  /**
   * Run `task` while the UI reports `label`. Nested calls keep the outer label
   * and only the outermost release clears the busy state.
   * @template T
   * @param {string} label
   * @param {(progress:(fraction:number)=>void)=>Promise<T>|T} task
   * @returns {Promise<T>}
   */
  async run(label, task) {
    this.count += 1;
    if (this.count === 1) {
      this.label = label;
      this.notify();
    }
    try {
      return await task((fraction) => this.report(label, fraction));
    } finally {
      this.count -= 1;
      if (this.count === 0) {
        this.label = '';
        this.notify();
      }
    }
  }

  /** Progress updates are forwarded to subscribers as `label (n%)`. */
  report(label, fraction) {
    if (this.count === 0) return;
    const percent = Math.max(0, Math.min(1, fraction)) * 100;
    this.label = `${label} ${percent.toFixed(0)}%`;
    this.notify();
  }
}
