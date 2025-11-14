<script lang="ts">
  import * as Card from "$lib/components/ui/card";
</script>

<div class="flex flex-1 flex-col gap-6 p-8">
  <div class="mx-auto w-full max-w-4xl space-y-6">
    <div>
      <h1 class="text-3xl font-bold mb-2">History</h1>
      <p class="text-muted-foreground">View and manage your past transcriptions</p>
    </div>

    <Card.Root>
      <Card.Content class="pt-6">
        <div class="flex flex-col items-center justify-center py-12 text-center">
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="48"
            height="48"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            class="mb-4 text-muted-foreground"
          >
            <circle cx="12" cy="12" r="10"></circle>
            <polyline points="12 6 12 12 16 14"></polyline>
          </svg>
          <h3 class="mb-2 text-lg font-semibold">No transcriptions yet</h3>
          <p class="mb-4 text-sm text-muted-foreground max-w-sm">
            Your transcription history will appear here once you start recording.
          </p>
          <p class="text-xs text-muted-foreground">
            History persistence coming soon...
          </p>
        </div>
      </Card.Content>
    </Card.Root>
  </div>
</div>
