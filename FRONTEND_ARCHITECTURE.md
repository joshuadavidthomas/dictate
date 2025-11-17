# Frontend Architecture Refactor

## Overview

This document describes the new frontend architecture that centralizes Tauri backend communication through a typed API layer and reactive Svelte 5 stores.

## Motivation

**Before:**
- Direct `invoke()` calls scattered across components
- Manual type casting (`as string`, `as boolean`)
- Duplicated event listeners in each component
- No single source of truth for state
- Hard to refactor when backend changes

**After:**
- ✅ Centralized, typed API layer
- ✅ Reactive Svelte 5 stores with runes
- ✅ Autocomplete and type safety
- ✅ Single source of truth for state
- ✅ Easy to refactor and maintain

## Architecture

```
src/lib/
├── api/              # Typed API layer for Tauri commands
│   ├── index.ts      # Re-exports all APIs
│   ├── types.ts      # Shared TypeScript types
│   ├── recording.ts  # Recording commands
│   ├── settings.ts   # Settings commands
│   ├── transcriptions.ts  # History commands
│   └── audio.ts      # Audio device commands
│
└── stores/           # Reactive Svelte 5 stores
    ├── index.ts      # Re-exports all stores
    ├── recording.svelte.ts     # Recording state
    ├── settings.svelte.ts      # Settings state
    └── transcriptions.svelte.ts  # History state
```

## API Layer

The API layer provides typed wrappers around Tauri `invoke()` commands:

### Example: `recordingApi`

```typescript
// lib/api/recording.ts
import { invoke } from '@tauri-apps/api/core';

export const recordingApi = {
  async toggle(): Promise<string> {
    return invoke('toggle_recording');
  },
  
  async getStatus(): Promise<RecordingStatus> {
    return invoke('get_status');
  }
};
```

### Benefits

1. **Type Safety**: No more manual type casting
2. **Autocomplete**: IDE knows all available commands
3. **Single Source**: Change command once, updates everywhere
4. **Documentation**: JSDoc comments on each method
5. **Error Handling**: Centralize error handling logic

## Svelte 5 Stores

Stores use Svelte 5's `$state` runes for automatic reactivity:

### Example: `recording` store

```typescript
// lib/stores/recording.svelte.ts
class RecordingStore {
  status = $state<RecordingStatus>('idle');
  transcriptionText = $state('');
  
  async toggle() {
    await recordingApi.toggle();
  }
  
  get isRecording() {
    return this.status === 'recording';
  }
}

export const recording = new RecordingStore();
```

### Benefits

1. **Reactive**: Auto-updates UI when state changes
2. **Event Listeners**: Centralized in store constructor
3. **Computed Properties**: `get isRecording()` etc.
4. **Methods**: Business logic encapsulated
5. **Clean Components**: Less code, more readable

## Usage in Components

### Before (Old Way)

```svelte
<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  
  let status = $state("idle");
  let transcriptionText = $state("");
  
  onMount(() => {
    listen("recording-started", () => {
      status = "recording";
    });
    
    invoke("get_status").then((s) => {
      status = s as string;
    });
  });
  
  async function toggle() {
    await invoke("toggle_recording");
  }
</script>

<button onclick={toggle}>
  {status === 'recording' ? 'Stop' : 'Start'}
</button>
```

### After (New Way)

```svelte
<script lang="ts">
  import { recording } from "$lib/stores";
  
  onMount(() => {
    recording.loadStatus();
  });
</script>

<button onclick={() => recording.toggle()}>
  {recording.isRecording ? 'Stop' : 'Start'}
</button>

{#if recording.transcriptionText}
  <p>{recording.transcriptionText}</p>
{/if}
```

## API Reference

### Recording API

```typescript
recordingApi.toggle() → Promise<string>
recordingApi.getStatus() → Promise<RecordingStatus>
recordingApi.getVersion() → Promise<string>
```

### Settings API

```typescript
settingsApi.getOutputMode() → Promise<OutputMode>
settingsApi.setOutputMode(mode) → Promise<void>
settingsApi.getWindowDecorations() → Promise<boolean>
settingsApi.setWindowDecorations(enabled) → Promise<void>
settingsApi.getOsdPosition() → Promise<OsdPosition>
settingsApi.setOsdPosition(position) → Promise<void>
settingsApi.checkConfigChanged() → Promise<boolean>
settingsApi.markConfigSynced() → Promise<void>
```

### Transcriptions API

```typescript
transcriptionsApi.list(limit?, offset?) → Promise<Transcription[]>
transcriptionsApi.search(query, limit?) → Promise<Transcription[]>
transcriptionsApi.getById(id) → Promise<Transcription | null>
transcriptionsApi.delete(id) → Promise<boolean>
transcriptionsApi.count() → Promise<number>
```

### Audio API

```typescript
audioApi.listDevices() → Promise<AudioDevice[]>
audioApi.getDevice() → Promise<string | null>
audioApi.setDevice(deviceName) → Promise<void>
audioApi.getSampleRate() → Promise<SampleRate>
audioApi.setSampleRate(sampleRate) → Promise<void>
audioApi.testDevice() → Promise<boolean>
audioApi.getAudioLevel() → Promise<AudioLevel>
```

## Store Reference

### `recording` Store

**State:**
- `status: RecordingStatus` - Current recording status
- `transcriptionText: string` - Latest transcription result

**Computed:**
- `isRecording: boolean` - True if currently recording
- `isTranscribing: boolean` - True if currently transcribing
- `isIdle: boolean` - True if idle

**Methods:**
- `toggle()` - Toggle recording on/off
- `loadStatus()` - Load initial status from backend

### `settings` Store

**State:**
- `outputMode: OutputMode` - Current output mode
- `windowDecorations: boolean` - Window decorations enabled
- `osdPosition: OsdPosition` - OSD position (top/bottom)
- `configChanged: boolean` - Config file changed externally

**Methods:**
- `load()` - Load all settings from backend
- `setOutputMode(mode)` - Set and save output mode
- `setWindowDecorations(enabled)` - Set and save window decorations
- `setOsdPosition(position)` - Set and save OSD position
- `checkConfigChanged()` - Check if config file changed
- `reloadFromFile()` - Reload settings from file
- `dismissConfigChanged()` - Save current UI values

### `transcriptions` Store

**State:**
- `items: Transcription[]` - List of transcriptions
- `totalCount: number` - Total count of transcriptions
- `loading: boolean` - Loading state
- `error: string` - Error message
- `searchQuery: string` - Current search query

**Methods:**
- `load(limit?, offset?)` - Load transcriptions
- `search(query)` - Search transcriptions
- `clearSearch()` - Clear search and reload
- `delete(id)` - Delete a transcription
- `getById(id)` - Get single transcription
- `refresh()` - Refresh current view

## Types

All types are defined in `src/lib/api/types.ts`:

```typescript
export type RecordingStatus = 'idle' | 'recording' | 'transcribing';
export type OutputMode = 'print' | 'copy' | 'insert';
export type OsdPosition = 'top' | 'bottom';
export type SampleRate = 16000 | 22050 | 44100 | 48000;

export interface Transcription { ... }
export interface AudioDevice { ... }
export interface SampleRateOption { ... }
// ... etc
```

## Migration Checklist

When adding new features:

1. ✅ Add types to `lib/api/types.ts`
2. ✅ Add API methods to appropriate `lib/api/*.ts` file
3. ✅ Add state/methods to appropriate `lib/stores/*.svelte.ts` if needed
4. ✅ Use stores in components via `import { storeName } from '$lib/stores'`

## Benefits Summary

| Aspect | Before | After |
|--------|--------|-------|
| **Type Safety** | Manual casts | Full TypeScript |
| **Autocomplete** | None | Full IDE support |
| **State Management** | Per-component | Centralized stores |
| **Event Listeners** | Duplicated | Centralized |
| **Refactoring** | Change in many files | Change in one place |
| **Testing** | Hard to mock | Easy to mock API layer |
| **Code Size** | ~50 lines/component | ~15 lines/component |

## Philosophy

**"Rust is the API, TypeScript is the Brain"** - but with a twist:

- ✅ Rust handles backend logic (what you wanted)
- ✅ TypeScript provides clean, typed access layer
- ✅ Svelte stores manage reactive UI state
- ✅ Components stay simple and declarative

This gives you the best of both worlds: business logic in Rust (fast, safe), with a clean, maintainable frontend layer.
