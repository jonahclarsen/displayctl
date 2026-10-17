<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  // on: built-in display on · armed: waiting for an external monitor · off: built-in display off
  let display = $state("on");
  const displayLabel = { on: "On", armed: "Waiting for monitor", off: "Off" };
  let caffeine = $state({ active: false, minutes: 120, remaining_seconds: null });
  let minutes = $state(120);
  let busy = $state(false);
  let error = $state("");
  let drag = $state(null);
  let suppressClick = false;

  const durationLabel = (value) => value % 60 === 0 ? `${value / 60} h` : `${Math.floor(value / 60)} h 30 m`;
  const clamp = (value) => Math.max(30, Math.min(1440, value));

  onMount(() => {
    let disposed = false;
    async function refreshCaffeine() {
      if (busy || drag) return;
      try {
        const status = await invoke("get_caffeine");
        if (!disposed && !busy && !drag) {
          caffeine = status;
          minutes = status.minutes;
        }
      } catch (e) {
        if (!disposed) error = String(e);
      }
    }
    refreshCaffeine();
    const timer = setInterval(refreshCaffeine, 1000);
    return () => { disposed = true; clearInterval(timer); };
  });

  async function updateCaffeine(active, duration = minutes) {
    if (busy) return;
    busy = true;
    error = "";
    try {
      caffeine = await invoke("set_caffeine", { active, minutes: duration });
      minutes = caffeine.minutes;
    } catch (e) {
      error = String(e);
      // A restart may have stopped the previous session before failing to spawn.
      try { caffeine = await invoke("get_caffeine"); } catch { /* keep the error visible */ }
      minutes = caffeine.minutes;
    } finally {
      busy = false;
    }
  }

  function startDrag(event) {
    if (event.button !== 0 || busy) return;
    suppressClick = false;
    event.currentTarget.setPointerCapture(event.pointerId);
    drag = { x: event.clientX, minutes, moved: false };
  }

  function moveDrag(event) {
    if (!drag) return;
    const delta = event.clientX - drag.x;
    if (Math.abs(delta) >= 6) drag.moved = true;
    if (drag.moved) minutes = clamp(drag.minutes + Math.round(delta / 18) * 30);
  }

  function endDrag(event) {
    if (!drag) return;
    suppressClick = drag.moved;
    const changed = minutes !== drag.minutes;
    drag = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (changed) updateCaffeine(caffeine.active);
  }

  function cancelDrag() {
    if (drag) minutes = drag.minutes;
    drag = null;
    suppressClick = true;
  }

  function durationClick() {
    if (suppressClick) { suppressClick = false; return; }
    updateCaffeine(!caffeine.active);
  }

  function durationKey(event) {
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      updateCaffeine(caffeine.active, clamp(minutes + (event.key === "ArrowRight" ? 30 : -30)));
    }
  }

  invoke("get_state").then((s) => (display = s));
  listen("state", (event) => (display = event.payload));

  function onkeydown(event) {
    if (event.key === "Escape") invoke("hide");
  }
</script>

<svelte:window {onkeydown} />

<button class="display {display}" onclick={() => invoke("toggle_display")} aria-label={`Toggle built-in display. ${displayLabel[display]}`} aria-pressed={display === "on"}>
  <svg viewBox="0 0 24 24" aria-hidden="true">
    <rect class="screen" x="5" y="4" width="14" height="12" rx="1.5" />
    <path d="m5 16-3 4h20l-3-4M10 20h4" />
    {#if display === "off"}<path class="screen-mark" d="m9 8 6 5m0-5-6 5" />{/if}
    {#if display === "armed"}<path class="screen-mark" d="M9 10h.01M12 10h.01M15 10h.01" />{/if}
  </svg>
  <span class="display-title">Built-in display</span>
  <span class="display-status"><span class="status-dot" aria-hidden="true"></span>{displayLabel[display]}</span>
</button>

<div class="caffeine" class:enabled={caffeine.active}>
  <button class="coffee-toggle" disabled={busy} onclick={() => updateCaffeine(!caffeine.active)} aria-label="Caffeinate" aria-pressed={caffeine.active}>
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M5 9h12v7a4 4 0 0 1-4 4H9a4 4 0 0 1-4-4Z" />
      <path d="M17 10h1a3 3 0 1 1 0 6h-1M8 3v3m4-4v4m4-3v3" />
    </svg>
    <span>Caffeinate</span>
    <span class="status-dot" aria-hidden="true"></span>
  </button>
  <button class="duration" class:dragging={drag?.moved} disabled={busy}
    onpointerdown={startDrag} onpointermove={moveDrag} onpointerup={endDrag} onpointercancel={cancelDrag}
    onclick={durationClick} onkeydown={durationKey}
    aria-label={`Duration ${durationLabel(minutes)}. Drag or use left and right arrows to adjust by 30 minutes. Click to ${caffeine.active ? "disable" : "enable"}.`}
    aria-pressed={caffeine.active}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m5 5-3 3 3 3m6-6 3 3-3 3M2 8h12" /></svg>
    <span>{durationLabel(minutes)}</span>
  </button>
  <span class="session-status" aria-live="polite">{error ? "Couldn’t update" : caffeine.active ? `${Math.max(1, Math.ceil((caffeine.remaining_seconds ?? 0) / 60))} min left` : "Off"}</span>
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<style>
  :global(:root) {
    --ink: #1d1d1f;
    --accent: #007aff;
    color-scheme: light dark;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --ink: #f5f5f7;
      --accent: #409cff;
    }
  }
  :global(html, body) {
    margin: 0;
    height: 100%;
    overflow: hidden;
    background: transparent;
    user-select: none;
    -webkit-user-select: none;
    cursor: default;
  }
  :global(#app) {
    height: 100%;
    box-sizing: border-box;
    padding: 8px;
    font-family: -apple-system, BlinkMacSystemFont, sans-serif;
  }

  button {
    all: unset;
    display: grid;
    place-items: center;
    color: var(--ink);
  }
  .display {
    box-sizing: border-box;
    width: 100%;
    height: 90px;
    margin-bottom: 8px;
    padding: 8px;
    align-content: center;
    gap: 3px;
  }
  .display-title { font-size: 12px; font-weight: 600; }
  .display-status { display: flex; align-items: center; gap: 5px; font-size: 10px; opacity: 0.65; }
  .display.on .screen { fill: color-mix(in srgb, var(--accent) 22%, transparent); }
  .display.on .status-dot { background: var(--accent); }
  .display.armed .status-dot { animation: breathe 2.4s ease-in-out infinite; }
  button:disabled { opacity: 0.6; }
  button:focus-visible svg {
    outline: 1.5px solid color-mix(in srgb, var(--ink) 35%, transparent);
    outline-offset: 6px;
    border-radius: 12px;
  }

  .display svg {
    width: 32px;
    height: 32px;
    overflow: visible;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.7;
    stroke-linecap: round;
    stroke-linejoin: round;
    transition: transform 160ms ease;
  }
  button:active svg {
    transform: scale(0.92);
  }

  .caffeine, .display {
    border-radius: 15px;
    border: 1px solid color-mix(in srgb, var(--ink) 12%, transparent);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    transition: background 180ms ease, border-color 180ms ease;
  }
  .caffeine { padding: 9px 8px 6px; }
  .caffeine.enabled {
    background: color-mix(in srgb, var(--accent) 24%, transparent);
    border-color: color-mix(in srgb, var(--accent) 65%, transparent);
  }
  .coffee-toggle { display: flex; gap: 6px; width: 100%; font-size: 12px; font-weight: 600; }
  .coffee-toggle svg, .duration svg { fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
  .coffee-toggle svg { width: 23px; height: 23px; }
  .status-dot { width: 6px; height: 6px; margin-left: auto; border-radius: 50%; background: color-mix(in srgb, var(--ink) 22%, transparent); }
  .enabled .status-dot { background: var(--accent); box-shadow: 0 0 6px color-mix(in srgb, var(--accent) 65%, transparent); }
  .duration { display: flex; justify-content: center; gap: 6px; width: 100%; height: 29px; margin-top: 4px; border-radius: 7px; cursor: ew-resize; touch-action: none; font-size: 14px; font-weight: 600; font-variant-numeric: tabular-nums; }
  .duration:hover, .duration.dragging { background: color-mix(in srgb, var(--ink) 9%, transparent); }
  .duration svg { width: 15px; height: 15px; opacity: 0.45; }
  .duration > svg, .duration > span { pointer-events: none; cursor: inherit; }
  .session-status { display: block; text-align: center; font-size: 10px; line-height: 14px; color: var(--ink); opacity: 0.6; }
  .duration:focus-visible, .coffee-toggle:focus-visible, .display:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 7px; }
  .error { position: absolute; inset: 5px; margin: 0; padding: 7px; border-radius: 8px; background: Canvas; color: CanvasText; font-size: 11px; pointer-events: none; }

  @keyframes breathe {
    50% {
      opacity: 0.5;
    }
  }
</style>
