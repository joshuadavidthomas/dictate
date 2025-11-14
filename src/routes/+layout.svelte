<script lang="ts">
	import '../app.css';
	import * as Sidebar from "$lib/components/ui/sidebar";
	import AppSidebar from "$lib/components/app-sidebar.svelte";
	
	let { children } = $props();
</script>

<Sidebar.Provider>
	<AppSidebar />
	<Sidebar.Inset>
		<main class="flex min-h-screen flex-1 flex-col">
			{@render children()}
		</main>
	</Sidebar.Inset>
</Sidebar.Provider>
