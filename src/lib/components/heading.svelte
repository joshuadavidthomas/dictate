<script lang="ts">
	import { cn } from "$lib/utils";

	type HeadingLevel = 1 | 2 | 3 | 4 | 5 | 6;
	type HeadingElement = "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
	type HTMLElementTag = keyof HTMLElementTagNameMap;

	interface Props {
		ref?: HTMLElement | null;
		level?: HeadingLevel;
		as?: HTMLElementTag;
		class?: string;
		children?: import("svelte").Snippet;
		[key: string]: any;
	}

	let {
		ref = $bindable(null),
		level = 1,
		as,
		class: className,
		children,
		...restProps
	}: Props = $props();

	// Determine the element to render
	const element = $derived(as || (`h${level}` as HeadingElement));

	// Get heading styles based on level
	const headingStyles = $derived.by(() => {
		const styles: Record<HeadingLevel, string> = {
			1: "scroll-m-20 text-4xl font-extrabold tracking-tight lg:text-5xl",
			2: "scroll-m-20 border-b pb-2 text-3xl font-semibold tracking-tight first:mt-0",
			3: "scroll-m-20 text-2xl font-semibold tracking-tight",
			4: "scroll-m-20 text-xl font-semibold tracking-tight",
			5: "scroll-m-20 text-lg font-semibold tracking-tight",
			6: "scroll-m-20 text-base font-semibold tracking-tight",
		};
		return styles[level];
	});
</script>

<svelte:element
	this={element}
	bind:this={ref}
	class={cn(headingStyles, className)}
	{...restProps}
>
	{@render children?.()}
</svelte:element>