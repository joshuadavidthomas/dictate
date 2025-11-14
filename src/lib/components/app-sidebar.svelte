<script lang="ts" module>
	import MicIcon from "@lucide/svelte/icons/mic";
	import HomeIcon from "@lucide/svelte/icons/home";
	import Settings2Icon from "@lucide/svelte/icons/settings-2";
	import InfoIcon from "@lucide/svelte/icons/info";
	import HistoryIcon from "@lucide/svelte/icons/history";

	// Navigation data for dictate app
	const data = {
		navMain: [
			{
				title: "Home",
				url: "/",
				icon: HomeIcon,
			},
			{
				title: "History",
				url: "/history",
				icon: HistoryIcon,
			},
			{
				title: "Settings",
				url: "/settings",
				icon: Settings2Icon,
			},
			{
				title: "About",
				url: "/about",
				icon: InfoIcon,
			},
		],
	};
</script>

<script lang="ts">
	import { invoke } from "@tauri-apps/api/core";
	import { onMount } from "svelte";
	import NavMain from "./nav-main.svelte";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import type { ComponentProps } from "svelte";

	let {
		ref = $bindable(null),
		collapsible = "icon",
		...restProps
	}: ComponentProps<typeof Sidebar.Root> = $props();

	let version = $state("...");

	onMount(async () => {
		try {
			version = await invoke<string>("get_version");
		} catch (err) {
			console.error("Failed to get version:", err);
			version = "0.1.0";
		}
	});
</script>

<Sidebar.Root {collapsible} {...restProps}>
	<Sidebar.Header>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<Sidebar.MenuButton size="lg" class="data-[state=open]:bg-sidebar-accent">
					<div class="flex items-center gap-2">
						<div class="bg-sidebar-primary text-sidebar-primary-foreground flex size-8 items-center justify-center rounded-lg">
							<MicIcon class="size-4" />
						</div>
						<div class="grid flex-1 text-left text-sm leading-tight">
							<span class="truncate font-semibold">dictate</span>
						</div>
					</div>
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Header>
	<Sidebar.Content>
		<NavMain items={data.navMain} />
	</Sidebar.Content>
	<Sidebar.Footer>
		<Sidebar.Menu>
			<Sidebar.MenuItem>
				<Sidebar.MenuButton size="sm" class="text-xs text-muted-foreground">
					<span>v{version}</span>
				</Sidebar.MenuButton>
			</Sidebar.MenuItem>
		</Sidebar.Menu>
	</Sidebar.Footer>
	<Sidebar.Rail />
</Sidebar.Root>
