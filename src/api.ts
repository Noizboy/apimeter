import { invoke } from '@tauri-apps/api/core';

export async function saveWindowPosition(x: number, y: number): Promise<void> {
  return invoke('save_window_position', { x, y });
}
