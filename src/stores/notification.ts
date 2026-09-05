import { writable } from 'svelte/store';

export type NotifyKind = 'idle' | 'ok' | 'error' | 'busy';
export type NotifyState = { msg: string; kind: NotifyKind; openFolder?: string; openFile?: string };

export const notification = writable<NotifyState>({ msg: 'Ready.', kind: 'idle' });
export const busy = writable(false);

let _dismissTimer: ReturnType<typeof setTimeout> | null = null;

export function notify(msg: string, kind: Exclude<NotifyKind, 'busy'> = 'idle', openFolder?: string, openFile?: string) {
  if (_dismissTimer) {
    clearTimeout(_dismissTimer);
    _dismissTimer = null;
  }
  notification.set({ msg, kind, openFolder, openFile });
  if (kind === 'ok' || kind === 'error') {
    _dismissTimer = setTimeout(() => {
      notification.set({ msg: 'Ready.', kind: 'idle' });
    }, 6000);
  }
}

export function setBusy(flag: boolean, msg = 'Working…') {
  busy.set(flag);
  if (flag) {
    notification.set({ msg, kind: 'busy' });
  }
}
