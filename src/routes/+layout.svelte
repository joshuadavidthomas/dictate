<script lang="ts">
	import AppSidebar from "$lib/components/app-sidebar.svelte";
	import * as Sidebar from "$lib/components/ui/sidebar";
	import { createRecordingState, createModelsState, createAppSettingsState, createThemeState } from "$lib/stores";
	import { modelsApi, settingsApi, audioApi } from "$lib/api";
	import { invoke } from '@tauri-apps/api/core';
	import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow';
	import type { AudioDevice, SampleRateOption } from "$lib/api/types";
	import { modelKey } from "$lib/stores";
	import { onMount, tick } from 'svelte';
	import '../app.css';

	let { children } = $props();

	// Create stores immediately (context must be set during component init)
	createRecordingState();
	const modelsState = createModelsState();
	const settingsState = createAppSettingsState();
	const themeState = createThemeState();

	// Load data asynchronously and update stores
	let ready = $state(false);

	onMount(() => {
		let cancelled = false;

		(async () => {
			try {
				const [
					modelsList,
					modelsSizes,
					outputMode,
					windowDecorations,
					osdPosition,
					audioDevices,
					currentDevice,
					sampleRateOptions,
					currentSampleRate,
					preferredModel,
					shortcut,
					shortcutCapabilities,
				] = await Promise.all([
					modelsApi.list(),
					modelsApi.getSizes(),
					settingsApi.getOutputMode(),
					settingsApi.getWindowDecorations(),
					settingsApi.getOsdPosition(),
					invoke("list_audio_devices") as Promise<AudioDevice[]>,
					audioApi.getDevice(),
					invoke("get_sample_rate_options_for_device", { deviceName: null }) as Promise<SampleRateOption[]>,
					audioApi.getSampleRate(),
					modelsApi.getPreferred(),
					settingsApi.getShortcut(),
					settingsApi.getShortcutCapabilities(),
				]);

				if (cancelled) return;

				modelsState.models = modelsList;
				const sizesRecord: Record<string, number> = {};
				for (const size of modelsSizes) {
					sizesRecord[modelKey(size)] = size.size_bytes;
				}
				modelsState.modelSizes = sizesRecord;
				modelsState.loading = false;

				settingsState.outputMode = outputMode;
				settingsState.windowDecorations = windowDecorations;
				settingsState.osdPosition = osdPosition;
				settingsState.currentDevice = currentDevice || "default";
				settingsState.availableDevices = audioDevices.map(d => d.name);
				settingsState.currentSampleRate = currentSampleRate;
				settingsState.availableSampleRates = sampleRateOptions;
				settingsState.preferredModel = preferredModel;
				settingsState.preferredModelValue = preferredModel 
					? `${preferredModel.engine}:${preferredModel.id}` 
					: '';
				settingsState.shortcut = shortcut;
				settingsState.shortcutCapabilities = shortcutCapabilities;

				ready = true;

				await tick();

				if (cancelled) return;

				const win = getCurrentWebviewWindow();
				await win.show();
				await win.setFocus();
			} catch (err) {
				console.error('Failed to load initial data:', err);
				
				ready = true;
				await tick();
				
				if (!cancelled) {
					const win = getCurrentWebviewWindow();
					await win.show();
					await win.setFocus();
				}
			}
		})();

		return () => {
			cancelled = true;
		};
	});
</script>

{#if ready}
	<Sidebar.Provider>
		<AppSidebar />
		<Sidebar.Inset>
			<main class="flex min-h-screen flex-1 flex-col">
				{@render children()}
			</main>
		</Sidebar.Inset>
	</Sidebar.Provider>
{/if}
