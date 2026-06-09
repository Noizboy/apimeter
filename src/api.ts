import { invoke } from '@tauri-apps/api/core';
import type { DashboardData } from './types';

export async function getDashboardData(): Promise<DashboardData> {
  return invoke<DashboardData>('get_dashboard_data');
}

export async function saveWindowPosition(x: number, y: number): Promise<void> {
  return invoke('save_window_position', { x, y });
}
