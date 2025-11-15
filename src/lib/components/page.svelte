<script lang="ts">
	import { cn } from "$lib/utils";

	interface Props {
		ref?: HTMLElement | null;
		class?: string;
		children?: import("svelte").Snippet;
		[key: string]: any;
	}

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: Props = $props();
</script>

<div
	bind:this={ref}
	class={cn("flex flex-1 flex-col gap-6 p-4", className)}
	{...restProps}
>
	{@render children?.()}
</div>
