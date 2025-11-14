# Simplification Plan: Remove Sockets, Use Tokio Channels

## Problem
- Socket connection timing issues
- iced OSD spawned in separate process, can't receive broadcasts
- Channel closed errors

## Solution: In-Process iced with Tokio Channels

Instead of:
```
Tauri → Unix Socket → Separate iced Process
```

Do:
```
Tauri → tokio::sync::broadcast → iced (same process, separate thread)
```

## Implementation
1. Keep broadcast::BroadcastServer (it already uses tokio channels!)
2. Remove socket listener entirely
3. Pass broadcast receiver directly to iced OSD
4. iced runs in separate thread but same process
5. No socket connection needed!

## Benefits
- No socket setup
- No connection timing issues  
- Instant communication
- Simpler code
- iced still uses layer-shell (Wayland-native)
