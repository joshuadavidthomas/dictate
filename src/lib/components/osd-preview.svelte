<script lang="ts">
  let { position = "top", class: className = "" }: { position?: "top" | "bottom"; class?: string } = $props();
  
  // OSD bar dimensions and positioning
  const OSD_BAR_WIDTH = 60;
  const OSD_BAR_HEIGHT = 6;
  const OSD_BAR_X = 70;
  const OSD_EDGE_OFFSET = 8;
  
  // Calculate OSD bar Y position based on prop
  // Screen area starts at y=0 and ends at y=120
  // Top: OSD at y=8, distance from top edge (0) = 8px
  // Bottom: OSD is 6px tall, to have 8px from bottom edge (120): 120 - 8 - 6 = 106
  const osdY = position === "top" ? OSD_EDGE_OFFSET : (120 - OSD_EDGE_OFFSET - OSD_BAR_HEIGHT);
  
  type WindowRect = { x: number; y: number; width: number; height: number };
  
  // Generate random window positions and sizes with overlap detection
  const random = (min: number, max: number) => Math.floor(Math.random() * (max - min + 1)) + min;
  
  // Check if two rectangles overlap significantly (more than 40% overlap is too much)
  const hasSignificantOverlap = (r1: WindowRect, r2: WindowRect): boolean => {
    const xOverlap = Math.max(0, Math.min(r1.x + r1.width, r2.x + r2.width) - Math.max(r1.x, r2.x));
    const yOverlap = Math.max(0, Math.min(r1.y + r1.height, r2.y + r2.height) - Math.max(r1.y, r2.y));
    const overlapArea = xOverlap * yOverlap;
    const r1Area = r1.width * r1.height;
    const r2Area = r2.width * r2.height;
    const minArea = Math.min(r1Area, r2Area);
    return overlapArea > minArea * 0.4; // More than 40% overlap
  };
  
  // Generate windows with reasonable spacing
  const generateWindow = (
    existingWindows: WindowRect[], 
    minX: number, maxX: number, 
    minY: number, maxY: number, 
    minW: number, maxW: number, 
    minH: number, maxH: number
  ): WindowRect => {
    let attempts = 0;
    while (attempts < 20) {
      const candidate: WindowRect = {
        x: random(minX, maxX),
        y: random(minY, maxY),
        width: random(minW, maxW),
        height: random(minH, maxH)
      };
      
      // Check if this overlaps too much with existing windows
      const hasIssue = existingWindows.some(w => hasSignificantOverlap(candidate, w));
      
      if (!hasIssue) {
        return candidate;
      }
      attempts++;
    }
    // If we can't find a good spot after 20 tries, return the last candidate anyway
    return {
      x: random(minX, maxX),
      y: random(minY, maxY),
      width: random(minW, maxW),
      height: random(minH, maxH)
    };
  };
  
  // Generate windows in order, each checking previous ones
  // Spread them across different regions: left, right, and center
  const window1: WindowRect = generateWindow([], 5, 35, 18, 40, 45, 55, 35, 45);
  const window2: WindowRect = generateWindow([window1], 90, 135, 15, 35, 55, 85, 45, 65);
  const window3: WindowRect = generateWindow([window1, window2], 35, 80, 35, 60, 40, 55, 30, 42);
  
  // Randomize dock icon colors
  const dockColors = ['fill-blue-500', 'fill-orange-500', 'fill-purple-500', 'fill-green-500', 'fill-pink-500', 'fill-cyan-500', 'fill-yellow-500', 'fill-emerald-500'];
  const shuffled = [...dockColors].sort(() => Math.random() - 0.5);
  const dockIcon1 = shuffled[0];
  const dockIcon2 = shuffled[1];
  const dockIcon3 = shuffled[2];
  const dockIcon4 = shuffled[3];
  
  // Randomize background pattern
  const patterns = ['topography', 'diagonal-lines', 'dots', 'circuit-board', 'wiggle'];
  const selectedPattern = patterns[Math.floor(Math.random() * patterns.length)];
  
  // Randomize background color with hex values for SVG patterns
  const bgColors = [
    '#3b82f6', // blue-500
    '#a855f7', // purple-500
    '#22c55e', // green-500
    '#f97316', // orange-500
    '#ec4899', // pink-500
    '#06b6d4', // cyan-500
    '#10b981', // emerald-500
    '#f43f5e'  // rose-500
  ];
  const selectedBgColor = bgColors[Math.floor(Math.random() * bgColors.length)];
</script>

<svg
  viewBox="0 0 200 120"
  class={className}
  xmlns="http://www.w3.org/2000/svg"
  style="pointer-events: none;"
>
  <!-- Screen background -->
  <rect
    x="0"
    y="0"
    width="200"
    height="120"
    rx="3"
    class="fill-background"
  />
  
  <defs>
    <!-- Wallpaper Patterns -->
    <!-- Topography -->
    <pattern id="topography" x="0" y="0" width="40" height="40" patternUnits="userSpaceOnUse">
      <path d="M0 20c5-2 10-3 15-2s10 4 15 4 10-2 15-4v2c-5 2-10 4-15 4s-10-2-15-4-10 0-15 2zm0 8c5-2 10-3 15-2s10 4 15 4 10-2 15-4v2c-5 2-10 4-15 4s-10-2-15-4-10 0-15 2z" fill={selectedBgColor} opacity="0.45"/>
    </pattern>
    
    <!-- Diagonal Lines -->
    <pattern id="diagonal-lines" x="0" y="0" width="8" height="8" patternUnits="userSpaceOnUse">
      <path d="M-2,2 l4,-4 M0,8 l8,-8 M6,10 l4,-4" stroke={selectedBgColor} stroke-width="0.5" opacity="0.45"/>
    </pattern>
    
    <!-- Dots -->
    <pattern id="dots" x="0" y="0" width="12" height="12" patternUnits="userSpaceOnUse">
      <circle cx="6" cy="6" r="1" fill={selectedBgColor} opacity="0.45"/>
    </pattern>
    
    <!-- Circuit Board -->
    <pattern id="circuit-board" x="0" y="0" width="24" height="24" patternUnits="userSpaceOnUse">
      <circle cx="12" cy="12" r="1" fill={selectedBgColor} opacity="0.45"/>
      <path d="M12 2v4m0 4v4m0 4v4M2 12h4m4 0h4m4 0h4" stroke={selectedBgColor} stroke-width="0.5" opacity="0.45"/>
    </pattern>
    
    <!-- Wiggle -->
    <pattern id="wiggle" x="0" y="0" width="20" height="12" patternUnits="userSpaceOnUse">
      <path d="M0 6c2.5-2 5-3 7.5-2s5 3 7.5 3 5-1 7.5-3" stroke={selectedBgColor} stroke-width="0.5" fill="none" opacity="0.45"/>
    </pattern>
  </defs>
  
  <!-- Wrap all desktop content in a group for blur effect -->
  <g filter="url(#desktop-blur)">
    <!-- Background -->
    <rect x="0" y="0" width="200" height="120" rx="3" class="fill-white dark:fill-gray-900" />
    <rect x="0" y="0" width="200" height="120" rx="3" fill="url(#{selectedPattern})" />

    <!-- System bar at top (always present) -->
  <g>
    <rect
      x="0"
      y="0"
      width="200"
      height="6"
      class="fill-gray-700"
      fill-opacity="0.9"
    />
    <line
      x1="0"
      y1="6"
      x2="200"
      y2="6"
      class="stroke-muted-foreground"
      stroke-width="0.5"
      stroke-opacity="0.1"
    />
    
    <!-- Left: System menu button (grid icon) -->
    <g transform="translate(4, 1)">
      <rect x="0" y="0" width="1.5" height="1.5" class="fill-white" fill-opacity="0.85" rx="0.3" />
      <rect x="2" y="0" width="1.5" height="1.5" class="fill-white" fill-opacity="0.85" rx="0.3" />
      <rect x="0" y="2" width="1.5" height="1.5" class="fill-white" fill-opacity="0.85" rx="0.3" />
      <rect x="2" y="2" width="1.5" height="1.5" class="fill-white" fill-opacity="0.85" rx="0.3" />
    </g>
    
    <!-- Center: time -->
    <text
      x="100"
      y="4"
      text-anchor="middle"
      class="fill-white"
      fill-opacity="0.9"
      font-family="monospace"
      font-size="4"
    >
      12:34
    </text>
    
    <!-- Right: System tray icons -->
    <g transform="translate(180, 1.5)">
      <!-- WiFi icon -->
      <path d="M 0 2 Q 2 0 4 2" class="stroke-white" stroke-width="0.5" fill="none" stroke-opacity="0.85" />
      <circle cx="2" cy="2.5" r="0.4" class="fill-white" fill-opacity="0.85" />
      
      <!-- Volume icon -->
      <g transform="translate(6, 0)">
        <path d="M 0 1 L 0 3 L 1 3 L 2 4 L 2 0 L 1 1 Z" class="fill-white" fill-opacity="0.85" />
        <path d="M 2.5 1 Q 3 2 2.5 3" class="stroke-white" stroke-width="0.4" fill="none" stroke-opacity="0.85" />
      </g>
      
      <!-- Battery icon -->
      <g transform="translate(11, 0.5)">
        <rect x="0" y="0" width="3" height="3" rx="0.3" class="stroke-white" stroke-width="0.4" fill="none" stroke-opacity="0.85" />
        <rect x="1" y="0" width="1" height="3" class="fill-white" fill-opacity="0.85" />
        <rect x="3" y="1" width="0.3" height="1" class="fill-white" fill-opacity="0.85" />
      </g>
    </g>
  </g>

  <!-- Window 1 - Random position/size -->
  <g>
    <rect
      x={window1.x}
      y={window1.y}
      width={window1.width}
      height={window1.height}
      rx="3"
      class="fill-background stroke-muted-foreground"
      stroke-width="0.5"
      stroke-opacity="0.4"
      filter="url(#shadow-sm)"
    />
    <rect x={window1.x} y={window1.y} width={window1.width} height="8" rx="3" class="fill-muted" fill-opacity="0.3" />
    <line x1={window1.x} y1={window1.y + 8} x2={window1.x + window1.width} y2={window1.y + 8} class="stroke-muted-foreground" stroke-width="0.5" stroke-opacity="0.2" />
    <circle cx={window1.x + 5} cy={window1.y + 4} r="1.5" class="fill-red-500" fill-opacity="0.7" />
    <circle cx={window1.x + 10} cy={window1.y + 4} r="1.5" class="fill-yellow-500" fill-opacity="0.7" />
    <circle cx={window1.x + 15} cy={window1.y + 4} r="1.5" class="fill-green-500" fill-opacity="0.7" />
    <rect x={window1.x + 4} y={window1.y + 12} width={window1.width * 0.5} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.2" />
    <rect x={window1.x + 4} y={window1.y + 18} width={window1.width * 0.4} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
    <rect x={window1.x + 4} y={window1.y + 24} width={window1.width * 0.55} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
  </g>

  <!-- Window 2 - Random position/size -->
  <g>
    <rect
      x={window2.x}
      y={window2.y}
      width={window2.width}
      height={window2.height}
      rx="3"
      class="fill-background stroke-muted-foreground"
      stroke-width="0.5"
      stroke-opacity="0.4"
      filter="url(#shadow-sm)"
    />
    <rect x={window2.x} y={window2.y} width={window2.width} height="8" rx="3" class="fill-muted" fill-opacity="0.3" />
    <line x1={window2.x} y1={window2.y + 8} x2={window2.x + window2.width} y2={window2.y + 8} class="stroke-muted-foreground" stroke-width="0.5" stroke-opacity="0.2" />
    <circle cx={window2.x + 5} cy={window2.y + 4} r="1.5" class="fill-red-500" fill-opacity="0.7" />
    <circle cx={window2.x + 10} cy={window2.y + 4} r="1.5" class="fill-yellow-500" fill-opacity="0.7" />
    <circle cx={window2.x + 15} cy={window2.y + 4} r="1.5" class="fill-green-500" fill-opacity="0.7" />
    <rect x={window2.x + 4} y={window2.y + 12} width={window2.width * 0.52} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.2" />
    <rect x={window2.x + 4} y={window2.y + 18} width={window2.width * 0.45} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
    <rect x={window2.x + 4} y={window2.y + 24} width={window2.width * 0.58} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
    <rect x={window2.x + 4} y={window2.y + 30} width={window2.width * 0.48} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
    <rect x={window2.x + 4} y={window2.y + 36} width={window2.width * 0.42} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
  </g>

  <!-- Window 3 - Random position/size -->
  <g>
    <rect
      x={window3.x}
      y={window3.y}
      width={window3.width}
      height={window3.height}
      rx="3"
      class="fill-background stroke-muted-foreground"
      stroke-width="0.5"
      stroke-opacity="0.4"
      filter="url(#shadow-sm)"
    />
    <rect x={window3.x} y={window3.y} width={window3.width} height="8" rx="3" class="fill-muted" fill-opacity="0.3" />
    <line x1={window3.x} y1={window3.y + 8} x2={window3.x + window3.width} y2={window3.y + 8} class="stroke-muted-foreground" stroke-width="0.5" stroke-opacity="0.2" />
    <circle cx={window3.x + 5} cy={window3.y + 4} r="1.5" class="fill-red-500" fill-opacity="0.7" />
    <circle cx={window3.x + 10} cy={window3.y + 4} r="1.5" class="fill-yellow-500" fill-opacity="0.7" />
    <circle cx={window3.x + 15} cy={window3.y + 4} r="1.5" class="fill-green-500" fill-opacity="0.7" />
    <rect x={window3.x + 4} y={window3.y + 12} width={window3.width * 0.5} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.2" />
    <rect x={window3.x + 4} y={window3.y + 18} width={window3.width * 0.38} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
    <rect x={window3.x + 4} y={window3.y + 24} width={window3.width * 0.6} height="2" rx="1" class="fill-muted-foreground" fill-opacity="0.15" />
  </g>

  <!-- Dock at bottom (always present) -->
  <g transform="translate(100, 110)">
    <!-- Dock background - pill shaped -->
    <rect
      x="-34"
      y="-8"
      width="68"
      height="16"
      rx="6"
      class="fill-gray-700"
      fill-opacity="0.85"
      filter="url(#dock-blur)"
    />
    
    <!-- Dock icons - grouped and centered -->
    <g transform="translate(-26, -5)">
      <rect x="0" y="0" width="10" height="10" rx="2" class={dockIcon1} fill-opacity="0.95" />
      <rect x="14" y="0" width="10" height="10" rx="2" class={dockIcon2} fill-opacity="0.95" />
      <rect x="28" y="0" width="10" height="10" rx="2" class={dockIcon3} fill-opacity="0.95" />
      <rect x="42" y="0" width="10" height="10" rx="2" class={dockIcon4} fill-opacity="0.95" />
    </g>
  </g>
  </g>  <!-- End of desktop content group with blur -->

  <!-- Full-screen overlay to dim content behind OSD bar -->
  <rect
    x="0"
    y="0"
    width="200"
    height="120"
    fill="#000000"
    fill-opacity="0.04"
  />

  <!-- OSD Bar (position based on prop) - RENDERED LAST so it appears on top -->
  <g>
    <!-- OSD background -->
    <rect
      x={OSD_BAR_X}
      y={osdY}
      width={OSD_BAR_WIDTH}
      height={OSD_BAR_HEIGHT}
      rx="3"
      class="fill-gray-800 dark:fill-gray-900"
      fill-opacity="0.95"
      filter="url(#shadow)"
    />
    
    <!-- Subtle highlight on top edge for glossy effect -->
    <rect
      x={OSD_BAR_X}
      y={osdY}
      width={OSD_BAR_WIDTH}
      height="1.5"
      rx="3"
      class="fill-white"
      fill-opacity="0.15"
    />
    
    <!-- Status dot (red for recording) - left side -->
    <circle 
      cx="75" 
      cy={osdY + 3} 
      r="1.2" 
      class="fill-red-500"
    >
      <animate attributeName="opacity" values="1;0.4;1" dur="1.5s" repeatCount="indefinite" />
    </circle>
    
    <!-- Timer text - multiple overlapping elements with visibility animation -->
    <g>
      <text x="107" y={osdY + 4.5} text-anchor="middle" class="fill-white" fill-opacity="0.85" font-family="monospace" font-size="3.5">
        0:03
        <animate attributeName="opacity" values="1;0;0;0;0;1" dur="5s" repeatCount="indefinite" />
      </text>
      <text x="107" y={osdY + 4.5} text-anchor="middle" class="fill-white" fill-opacity="0.85" font-family="monospace" font-size="3.5">
        0:04
        <animate attributeName="opacity" values="0;1;0;0;0;0" dur="5s" repeatCount="indefinite" />
      </text>
      <text x="107" y={osdY + 4.5} text-anchor="middle" class="fill-white" fill-opacity="0.85" font-family="monospace" font-size="3.5">
        0:05
        <animate attributeName="opacity" values="0;0;1;0;0;0" dur="5s" repeatCount="indefinite" />
      </text>
      <text x="107" y={osdY + 4.5} text-anchor="middle" class="fill-white" fill-opacity="0.85" font-family="monospace" font-size="3.5">
        0:06
        <animate attributeName="opacity" values="0;0;0;1;0;0" dur="5s" repeatCount="indefinite" />
      </text>
      <text x="107" y={osdY + 4.5} text-anchor="middle" class="fill-white" fill-opacity="0.85" font-family="monospace" font-size="3.5">
        0:07
        <animate attributeName="opacity" values="0;0;0;0;1;0" dur="5s" repeatCount="indefinite" />
      </text>
    </g>
    
    <!-- Mini spectrum bars (5 bars) - right side -->
    <g transform={`translate(115, ${osdY + 1})`}>
      <rect x="0" y="2" width="1" height="2" class="fill-red-500" fill-opacity="0.9">
        <animate attributeName="height" values="2;4;1;3;2" dur="0.8s" repeatCount="indefinite" />
        <animate attributeName="y" values="2;0;3;1;2" dur="0.8s" repeatCount="indefinite" />
      </rect>
      <rect x="2" y="1" width="1" height="3" class="fill-red-500" fill-opacity="0.9">
        <animate attributeName="height" values="3;2;4;2;3" dur="0.9s" repeatCount="indefinite" />
        <animate attributeName="y" values="1;2;0;2;1" dur="0.9s" repeatCount="indefinite" />
      </rect>
      <rect x="4" y="0" width="1" height="4" class="fill-red-500" fill-opacity="0.9">
        <animate attributeName="height" values="4;3;4;2;4" dur="0.7s" repeatCount="indefinite" />
        <animate attributeName="y" values="0;1;0;2;0" dur="0.7s" repeatCount="indefinite" />
      </rect>
      <rect x="6" y="1.5" width="1" height="2.5" class="fill-red-500" fill-opacity="0.9">
        <animate attributeName="height" values="2.5;4;2;3;2.5" dur="0.85s" repeatCount="indefinite" />
        <animate attributeName="y" values="1.5;0;2;1;1.5" dur="0.85s" repeatCount="indefinite" />
      </rect>
      <rect x="8" y="2" width="1" height="2" class="fill-red-500" fill-opacity="0.9">
        <animate attributeName="height" values="2;3;1;3.5;2" dur="0.95s" repeatCount="indefinite" />
        <animate attributeName="y" values="2;1;3;0.5;2" dur="0.95s" repeatCount="indefinite" />
      </rect>
    </g>
  </g>

  <!-- Filters -->
  <defs>
    <filter id="shadow" x="-100%" y="-100%" width="300%" height="300%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="2" />
      <feOffset dx="0" dy="2" result="offsetblur" />
      <feComponentTransfer>
        <feFuncA type="linear" slope="0.45" />
      </feComponentTransfer>
      <feMerge>
        <feMergeNode />
        <feMergeNode in="SourceGraphic" />
      </feMerge>
    </filter>
    <filter id="shadow-sm" x="-50%" y="-50%" width="200%" height="200%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="1" />
      <feOffset dx="0" dy="1" result="offsetblur" />
      <feComponentTransfer>
        <feFuncA type="linear" slope="0.2" />
      </feComponentTransfer>
      <feMerge>
        <feMergeNode />
        <feMergeNode in="SourceGraphic" />
      </feMerge>
    </filter>
    <filter id="dock-blur" x="-20%" y="-20%" width="140%" height="140%">
      <feGaussianBlur in="SourceGraphic" stdDeviation="0.5" />
    </filter>
    
    <filter id="desktop-blur" x="-10%" y="-10%" width="120%" height="120%">
      <feGaussianBlur in="SourceGraphic" stdDeviation="0.85" />
    </filter>
  </defs>
</svg>
