<script lang="ts">
	import AppSidebar from "$lib/components/app-sidebar.svelte";
	import * as Sidebar from "$lib/components/ui/sidebar";
	import { setModelsState } from "$lib/stores/transcription-models-context.svelte";
	import { TranscriptionModelsState } from "$lib/stores/transcription-models.svelte";
	import { listen } from '@tauri-apps/api/event';
	import { onMount } from 'svelte';
	import '../app.css';

	const modelsState = new TranscriptionModelsState();
	setModelsState(modelsState);

	let { children } = $props();

	onMount(() => {
		let unlisten: (() => void) | undefined;

		(async () => {
			try {
				unlisten = await listen('model-download-progress', (event) => {
					modelsState.updateDownloadProgress(
						// eslint-disable-next-line @typescript-eslint/no-explicit-any
						(event.payload as any) as {
							id: import('$lib/api/types').ModelId;
							downloaded_bytes: number;
							total_bytes: number;
							phase: string;
						}
					);
				});
			} catch (err) {
				console.error('Failed to listen for model-download-progress', err);
			}
		})();

		return () => {
			if (unlisten) unlisten();
		};
	});
</script>

<Sidebar.Provider>
	<AppSidebar />
	<Sidebar.Inset>
		<main class="flex min-h-screen flex-1 flex-col">
			{@render children()}
		</main>
	</Sidebar.Inset>
</Sidebar.Provider>
