/**
 * Centralized API for all Tauri backend commands
 * 
 * Usage:
 * import { recordingApi, settingsApi, transcriptionsApi } from '$lib/api';
 * 
 * const status = await recordingApi.getStatus();
 * await settingsApi.setOutputMode('copy');
 * const transcriptions = await transcriptionsApi.list();
 */

export * from './types';
export * from './recording';
export * from './settings';
export * from './transcriptions';
export * from './audio';
