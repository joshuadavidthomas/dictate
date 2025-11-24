<script lang="ts">
	import AppSidebar from "$lib/components/app-sidebar.svelte";
	import * as Sidebar from "$lib/components/ui/sidebar";
	import { createRecordingState, createModelsState, createAppSettingsState, type InitialSettingsData } from "$lib/stores";
	import { modelsApi, settingsApi } from "$lib/api";
	import { invoke } from '@tauri-apps/api/core';
	import type { AudioDevice, SampleRateOption, ModelInfo, ModelSize } from "$lib/api/types";
	import { modelKey } from "$lib/stores";
	import '../app.css';

	let { children } = $props();

	// Create stores immediately (context must be set during component init)
	createRecordingState();
	const modelsState = createModelsState();
	const settingsState = createAppSettingsState();

	// Load data asynchronously and update stores
	let ready = $state(false);

	Promise.all([
		// Models data
		modelsApi.list(),
		modelsApi.getSizes(),
		
		// Settings data
		settingsApi.getOutputMode(),
		settingsApi.getWindowDecorations(),
		settingsApi.getOsdPosition(),
		invoke("list_audio_devices") as Promise<AudioDevice[]>,
		invoke("get_audio_device") as Promise<string | null>,
		invoke("get_sample_rate_options_for_device", { deviceName: null }) as Promise<SampleRateOption[]>,
		invoke("get_sample_rate") as Promise<number>,
		modelsApi.getPreferred(),
	]).then(([
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
	]) => {
		// Update models state
		modelsState.models = modelsList;
		const sizesRecord: Record<string, number> = {};
		for (const size of modelsSizes) {
			sizesRecord[modelKey(size.id)] = size.size_bytes;
		}
		modelsState.modelSizes = sizesRecord;
		modelsState.loading = false;

		// Update settings state
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

		ready = true;
	}).catch(err => {
		console.error('Failed to load initial data:', err);
		ready = true; // Show UI anyway, stores will load async
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
