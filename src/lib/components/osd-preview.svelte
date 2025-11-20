<script lang="ts">
  let { position = "top", class: className = "" }: { position?: "top" | "bottom"; class?: string } = $props();

  // Randomize background gradient
  const bgGradients = [
    'from-blue-500 to-purple-600',
    'from-purple-500 to-pink-600',
    'from-green-500 to-emerald-600',
    'from-orange-500 to-red-600',
    'from-cyan-500 to-blue-600',
    'from-pink-500 to-rose-600',
    'from-emerald-500 to-green-600',
  ];
  const selectedGradient = bgGradients[Math.floor(Math.random() * bgGradients.length)];

  // Randomize dock icon colors
  const iconColors = ['bg-blue-500', 'bg-orange-500', 'bg-purple-500', 'bg-green-500', 'bg-pink-500', 'bg-cyan-500', 'bg-yellow-500', 'bg-emerald-500'];
  const shuffled = [...iconColors].sort(() => Math.random() - 0.5);
  const dockIcon1 = shuffled[0];
  const dockIcon2 = shuffled[1];
  const dockIcon3 = shuffled[2];
  const dockIcon4 = shuffled[3];

  // Random window positions (simplified, using percentages)
  const random = (min: number, max: number) => Math.floor(Math.random() * (max - min + 1)) + min;
  const window1 = {
    left: random(5, 25) + '%',
    top: random(15, 30) + '%',
    width: random(35, 45) + '%',
    height: random(30, 40) + '%',
  };
  const window2 = {
    left: random(45, 65) + '%',
    top: random(20, 35) + '%',
    width: random(25, 35) + '%',
    height: random(35, 45) + '%',
  };

  // OSD position class
  const osdPositionClass = position === "top" ? "top-4" : "bottom-4";

  // Hover state controls whether animation runs
  let isHovered = $state(false);

  // Timer + spectrum animation (Svelte 5 runes)
  let timerSeconds = $state(5);
  let timerLabel = $derived(
    `${Math.floor(timerSeconds / 60)}:${String(timerSeconds % 60).padStart(2, "0")}`
  );

  let spectrumLevels = $state([0.4, 0.8, 1.0, 0.7, 0.5]);

  // Start intervals only while hovered
  $effect(() => {
    if (!isHovered) return;

    const timerId = setInterval(() => {
      timerSeconds = (timerSeconds + 1) % 3600;
    }, 1000);

    const spectrumId = setInterval(() => {
      spectrumLevels = spectrumLevels.map(() => 0.3 + Math.random() * 0.7);
    }, 220);

    return () => {
      clearInterval(timerId);
      clearInterval(spectrumId);
    };
  });
</script>


<!-- <div class="absolute inset-0 opacity-30" style="background-image: radial-gradient(circle at 2px 2px, rgba(255,255,255,0.15) 1px, transparent 0); background-size: 20px 20px;"></div> -->
<div
  class={`@container relative grid grid-rows-[5%_1fr] aspect-[5/3] overflow-hidden rounded-lg ${className}`}
  role="img"
  aria-label="On-screen display preview"
  onmouseenter={() => (isHovered = true)}
  onmouseleave={() => (isHovered = false)}
>
  <div class="bg-gray-800/90 border-b border-white/10 grid grid-cols-[2rem_1fr_2rem] items-center px-2">
    <span class="size-2 rounded bg-white/85"></span>
    <span class="mx-auto text-[0.5rem] text-white/90 font-mono">12:34</span>
    <div class="flex gap-1 items-center justify-end">
      <span class="size-1 bg-white/85"></span>
      <span class="size-1 bg-white/85"></span>
      <span class="size-1 bg-white/85"></span>
      <span class="size-1 bg-white/85"></span>
    </div>
  </div>

  <div class={`relative bg-gradient-to-br ${selectedGradient}`}>
    <div
      class="absolute bg-white/90 dark:bg-gray-800/90 rounded-md shadow-lg border border-black/10"
      style={`left: ${window1.left}; top: ${window1.top}; width: ${window1.width}; height: ${window1.height};`}
    >
      <div class="h-[15%] bg-gray-100/80 dark:bg-gray-700/80 border-b border-gray-300/50 dark:border-gray-600/50 rounded-t-md flex items-center gap-1 px-1.5">
        <div class="w-1.5 h-1.5 rounded-full bg-red-500/70"></div>
        <div class="w-1.5 h-1.5 rounded-full bg-yellow-500/70"></div>
        <div class="w-1.5 h-1.5 rounded-full bg-green-500/70"></div>
      </div>
      <div class="p-1.5 space-y-1">
        <div class="h-1 bg-gray-300/50 dark:bg-gray-600/50 rounded w-3/4"></div>
        <div class="h-1 bg-gray-300/40 dark:bg-gray-600/40 rounded w-2/3"></div>
        <div class="h-1 bg-gray-300/40 dark:bg-gray-600/40 rounded w-4/5"></div>
      </div>
    </div>

    <div
      class="absolute bg-white/90 dark:bg-gray-800/90 rounded-md shadow-lg border border-black/10"
      style={`left: ${window2.left}; top: ${window2.top}; width: ${window2.width}; height: ${window2.height};`}
    >
      <div class="h-[15%] bg-gray-100/80 dark:bg-gray-700/80 border-b border-gray-300/50 dark:border-gray-600/50 rounded-t-md flex items-center gap-1 px-1.5">
        <div class="w-1.5 h-1.5 rounded-full bg-red-500/70"></div>
        <div class="w-1.5 h-1.5 rounded-full bg-yellow-500/70"></div>
        <div class="w-1.5 h-1.5 rounded-full bg-green-500/70"></div>
      </div>
      <div class="p-1.5 space-y-1">
        <div class="h-1 bg-gray-300/50 dark:bg-gray-600/50 rounded w-2/3"></div>
        <div class="h-1 bg-gray-300/40 dark:bg-gray-600/40 rounded w-3/4"></div>
        <div class="h-1 bg-gray-300/40 dark:bg-gray-600/40 rounded w-1/2"></div>
      </div>
    </div>

    <div class="absolute bg-gray-800/80 bottom-1 left-1/2 -translate-x-1/2 flex gap-1.5 px-3 py-1.5 rounded-xl shadow-lg">
      <div class={`size-4 @xs:size-6 ${dockIcon1} rounded`}></div>
      <div class={`size-4 @xs:size-6 ${dockIcon2} rounded`}></div>
      <div class={`size-4 @xs:size-6 ${dockIcon3} rounded`}></div>
      <div class={`size-4 @xs:size-6 ${dockIcon4} rounded`}></div>
    </div>
  </div>

  <div class={`absolute z-10 ${osdPositionClass} left-1/2 -translate-x-1/2`}>
    <div class="relative flex justify-between items-center w-32 h-3 px-1.5 rounded-md
                bg-gradient-to-b from-gray-900/90 via-gray-900/85 to-black/90
                shadow-[0_3px_8px_rgba(0,0,0,0.55)] border border-white/10">
      <!-- Faint inner shadow -->
      <div class="pointer-events-none absolute inset-0 rounded-md shadow-[inset_0_0_4px_rgba(0,0,0,0.35)]"></div>
      <!-- Subtle glossy top highlight -->
      <div class="pointer-events-none absolute inset-x-0 top-0 h-[1px]
                  bg-gradient-to-b from-white/30 via-white/10 to-transparent rounded-t-md"></div>

      <div>
        <div class="size-1.5 bg-red-500 rounded-full"></div>
      </div>
      <div class="flex items-center gap-2">
        <span class="text-white/85 text-[0.5rem] font-mono">{timerLabel}</span>
        <div class="flex gap-0.5 items-end h-2">
          {#each spectrumLevels as level}
            <div
              class="w-0.5 bg-red-500 rounded-sm transition-[height] duration-150 ease-out"
              style={`height: ${Math.round(level * 100)}%`}
            ></div>
          {/each}
        </div>
      </div>
    </div>
  </div>
  <div
    class={`absolute inset-0 transition-all duration-150
            ${isHovered ? 'bg-gray-900/15 backdrop-blur-[1px]' : 'bg-gray-900/25 backdrop-blur-[2px]'}`}
  ></div>
</div>
