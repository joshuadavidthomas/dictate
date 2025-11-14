<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";

  let status = $state("idle");
  let transcriptionText = $state("");
  let outputMode = $state("print");
  let recording = $derived(status === "recording");
  let transcribing = $derived(status === "transcribing");

  onMount(() => {
    // Listen for status updates from Rust
    const unsubscribe1 = listen("recording-started", () => {
      status = "recording";
      transcriptionText = "";
    });
    
    const unsubscribe2 = listen("recording-stopped", () => {
      status = "transcribing";
    });
    
    const unsubscribe3 = listen("transcription-complete", () => {
      status = "idle";
    });

    const unsubscribe4 = listen("transcription-result", (event: any) => {
      transcriptionText = event.payload.text;
    });

    // Get initial status and output mode
    invoke("get_status").then((s) => {
      status = s as string;
    });
    
    invoke("get_output_mode").then((mode) => {
      outputMode = mode as string;
    });

    return () => {
      unsubscribe1.then(f => f());
      unsubscribe2.then(f => f());
      unsubscribe3.then(f => f());
      unsubscribe4.then(f => f());
    };
  });

  async function toggle() {
    try {
      await invoke("toggle_recording");
    } catch (err) {
      console.error("Toggle failed:", err);
    }
  }
  
  async function handleOutputModeChange() {
    try {
      await invoke("set_output_mode", { mode: outputMode });
    } catch (err) {
      console.error("Failed to set output mode:", err);
    }
  }
</script>

<main class="container">
  <h1>dictate</h1>
  <p class="subtitle">Voice transcription for Linux</p>

  <div class="status">
    <div class="status-indicator" class:recording class:transcribing>
      {#if recording}
        Recording...
      {:else if transcribing}
        Transcribing...
      {:else}
        Ready
      {/if}
    </div>
  </div>

  <button 
    class="toggle-btn"
    class:recording
    onclick={toggle}
    disabled={transcribing}
  >
    {#if recording}
      Stop Recording
    {:else if transcribing}
      Processing...
    {:else}
      Start Recording
    {/if}
  </button>

  {#if transcriptionText}
    <div class="result">
      <h3>Transcription:</h3>
      <p class="transcription-text">{transcriptionText}</p>
    </div>
  {/if}

  <div class="settings">
    <h3>Output Mode</h3>
    <div class="mode-selector">
      <label class:selected={outputMode === "print"}>
        <input 
          type="radio" 
          bind:group={outputMode} 
          value="print" 
          onchange={handleOutputModeChange} 
        />
        <span>Print to console</span>
      </label>
      <label class:selected={outputMode === "copy"}>
        <input 
          type="radio" 
          bind:group={outputMode} 
          value="copy" 
          onchange={handleOutputModeChange} 
        />
        <span>Copy to clipboard</span>
      </label>
      <label class:selected={outputMode === "insert"}>
        <input 
          type="radio" 
          bind:group={outputMode} 
          value="insert" 
          onchange={handleOutputModeChange} 
        />
        <span>Insert at cursor</span>
      </label>
    </div>
    {#if outputMode === "copy"}
      <p class="help-text">Requires: <code>wl-copy</code> (Wayland) or <code>xclip</code> (X11)</p>
    {:else if outputMode === "insert"}
      <p class="help-text">Requires: <code>wtype</code> (Wayland) or <code>xdotool</code> (X11)</p>
    {/if}
  </div>

  <div class="info">
    <p>Press the button or use your configured hotkey to toggle recording.</p>
    <p class="hint">Tip: Bind a system hotkey to run: <code>dictate toggle</code></p>
  </div>
</main>

<style>
  :root {
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, Cantarell, sans-serif;
    color: #f6f6f6;
    background-color: #1a1a1a;
  }

  .container {
    max-width: 600px;
    margin: 0 auto;
    padding: 2rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2rem;
  }

  h1 {
    font-size: 3rem;
    font-weight: 700;
    margin: 0;
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  .subtitle {
    margin: -1rem 0 0 0;
    color: #888;
    font-size: 1.1rem;
  }

  .status {
    width: 100%;
    display: flex;
    justify-content: center;
    padding: 2rem 0;
  }

  .status-indicator {
    padding: 1rem 2rem;
    border-radius: 8px;
    background: #2a2a2a;
    border: 2px solid #3a3a3a;
    font-size: 1.2rem;
    font-weight: 500;
    transition: all 0.3s ease;
  }

  .status-indicator.recording {
    background: rgba(239, 68, 68, 0.1);
    border-color: #ef4444;
    color: #ef4444;
    animation: pulse 2s ease-in-out infinite;
  }

  .status-indicator.transcribing {
    background: rgba(59, 130, 246, 0.1);
    border-color: #3b82f6;
    color: #3b82f6;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.5; }
  }

  .toggle-btn {
    padding: 1.2rem 3rem;
    font-size: 1.2rem;
    font-weight: 600;
    border-radius: 12px;
    border: none;
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    color: white;
    cursor: pointer;
    transition: all 0.3s ease;
    box-shadow: 0 4px 12px rgba(102, 126, 234, 0.4);
  }

  .toggle-btn:hover:not(:disabled) {
    transform: translateY(-2px);
    box-shadow: 0 6px 20px rgba(102, 126, 234, 0.6);
  }

  .toggle-btn:active:not(:disabled) {
    transform: translateY(0);
  }

  .toggle-btn.recording {
    background: linear-gradient(135deg, #ef4444 0%, #dc2626 100%);
    box-shadow: 0 4px 12px rgba(239, 68, 68, 0.4);
  }

  .toggle-btn.recording:hover {
    box-shadow: 0 6px 20px rgba(239, 68, 68, 0.6);
  }

  .toggle-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .info {
    text-align: center;
    color: #888;
    max-width: 500px;
  }

  .info p {
    margin: 0.5rem 0;
  }

  .hint {
    font-size: 0.9rem;
    color: #666;
  }

  code {
    background: #2a2a2a;
    padding: 0.2rem 0.5rem;
    border-radius: 4px;
    font-family: 'Monaco', 'Courier New', monospace;
    color: #a78bfa;
  }

  .result {
    width: 100%;
    max-width: 600px;
    padding: 1.5rem;
    background: #2a2a2a;
    border: 2px solid #3a3a3a;
    border-radius: 12px;
  }

  .result h3 {
    margin: 0 0 1rem 0;
    font-size: 1.1rem;
    color: #a78bfa;
  }

  .transcription-text {
    margin: 0;
    line-height: 1.6;
    color: #f6f6f6;
  }

  .settings {
    width: 100%;
    max-width: 600px;
    padding: 1.5rem;
    background: rgba(255, 255, 255, 0.05);
    border: 2px solid #3a3a3a;
    border-radius: 12px;
  }

  .settings h3 {
    margin: 0 0 1rem 0;
    font-size: 1.1rem;
    color: #a78bfa;
  }

  .mode-selector {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .mode-selector label {
    display: flex;
    align-items: center;
    padding: 0.75rem 1rem;
    background: #2a2a2a;
    border: 2px solid #3a3a3a;
    border-radius: 8px;
    cursor: pointer;
    transition: all 0.2s ease;
  }

  .mode-selector label:hover {
    border-color: #667eea;
    background: rgba(102, 126, 234, 0.1);
  }

  .mode-selector label.selected {
    border-color: #667eea;
    background: rgba(102, 126, 234, 0.15);
  }

  .mode-selector input[type="radio"] {
    margin-right: 0.75rem;
    width: 18px;
    height: 18px;
    cursor: pointer;
  }

  .mode-selector span {
    font-size: 1rem;
    color: #f6f6f6;
  }

  .help-text {
    margin-top: 1rem;
    font-size: 0.875rem;
    color: #888;
    line-height: 1.5;
  }
</style>
