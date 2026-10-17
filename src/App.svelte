<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  // on: built-in display on · armed: waiting for an external monitor · off: built-in display off
  let display = $state("on");
  const lidAngle = { on: 100, armed: 40, off: 1.5 };
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

<button class="display {display}" onclick={() => invoke("toggle_display")} aria-label="Toggle built-in display">
  <svg viewBox="0 0 18 18" aria-hidden="true">
    <g class="rays">
      <line x1="7.3" y1="9.5" x2="10.3" y2="10.6" />
      <line x1="7.4" y1="7.5" x2="10.6" y2="7" />
      <line x1="6.6" y1="5.8" x2="9.1" y2="3.7" />
    </g>
    <line class="base" x1="3.6" y1="15.3" x2="16.2" y2="15.3" />
    <line class="lid" x1="4.6" y1="13.4" x2="15.2" y2="13.4" style:transform="rotate(-{lidAngle[display]}deg)" />
  </svg>
</button>

<div class="caffeine" class:enabled={caffeine.active}>
  <button class="coffee-toggle" disabled={busy} onclick={() => updateCaffeine(!caffeine.active)} aria-label="Caffeinate" aria-pressed={caffeine.active} title="Keep the Mac and display awake">
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
    aria-pressed={caffeine.active} title="Drag left or right in 30-minute steps · click to toggle">
    <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m5 5-3 3 3 3m6-6 3 3-3 3M2 8h12" /></svg>
    <span>{durationLabel(minutes)}</span>
  </button>
  <span class="session-status" aria-live="polite">{error ? "Couldn’t update" : caffeine.active ? `${Math.max(1, Math.ceil((caffeine.remaining_seconds ?? 0) / 60))} min left` : "Off"}</span>
</div>
{#if error}<p class="error" role="alert" title={error}>{error}</p>{/if}

<style>
  :global(:root) {
    --ink: #1d1d1f;
    --glow: #f5a623;
    color-scheme: light dark;
  }
  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --ink: #f5f5f7;
      --glow: #ffc35a;
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
  .display { width: 100%; height: 90px; }
  button:disabled { opacity: 0.6; }
  button:focus-visible svg {
    outline: 1.5px solid color-mix(in srgb, var(--ink) 35%, transparent);
    outline-offset: 6px;
    border-radius: 12px;
  }

  .display svg {
    width: 64px;
    height: 64px;
    overflow: visible;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.7;
    stroke-linecap: round;
    transition: transform 160ms ease;
  }
  button:active svg {
    transform: scale(0.92);
  }

  .caffeine {
    border-radius: 15px;
    border: 1px solid color-mix(in srgb, var(--ink) 12%, transparent);
    background: color-mix(in srgb, var(--ink) 4%, transparent);
    transition: background 180ms ease, border-color 180ms ease;
    padding: 9px 8px 6px;
  }
  .caffeine.enabled {
    background: color-mix(in srgb, var(--glow) 24%, transparent);
    border-color: color-mix(in srgb, var(--glow) 65%, transparent);
  }
  .coffee-toggle { display: flex; gap: 6px; width: 100%; font-size: 12px; font-weight: 600; }
  .coffee-toggle svg, .duration svg { fill: none; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; }
  .coffee-toggle svg { width: 23px; height: 23px; }
  .status-dot { width: 6px; height: 6px; margin-left: auto; border-radius: 50%; background: color-mix(in srgb, var(--ink) 22%, transparent); }
  .enabled .status-dot { background: var(--glow); box-shadow: 0 0 6px color-mix(in srgb, var(--glow) 65%, transparent); }
  .duration { display: flex; justify-content: center; gap: 6px; width: 100%; height: 29px; margin-top: 4px; border-radius: 7px; cursor: ew-resize; touch-action: none; font-size: 14px; font-weight: 600; font-variant-numeric: tabular-nums; }
  .duration:hover, .duration.dragging { background: color-mix(in srgb, var(--ink) 9%, transparent); }
  .duration svg { width: 15px; height: 15px; opacity: 0.45; }
  .session-status { display: block; text-align: center; font-size: 10px; line-height: 14px; color: var(--ink); opacity: 0.6; }
  .duration:focus-visible, .coffee-toggle:focus-visible { outline: 2px solid var(--glow); outline-offset: 2px; border-radius: 7px; }
  .error { position: absolute; inset: 5px; margin: 0; padding: 7px; border-radius: 8px; background: Canvas; color: CanvasText; font-size: 11px; pointer-events: none; }

  .lid {
    transform-box: view-box;
    transform-origin: 4.6px 13.4px;
    transition: transform 520ms cubic-bezier(0.34, 1.4, 0.5, 1);
  }

  .rays {
    stroke: var(--glow);
    stroke-width: 1.3;
    opacity: 0;
    transform: translateX(-1.5px);
    transition: opacity 300ms ease, transform 400ms ease;
  }
  .on .rays {
    opacity: 1;
    transform: none;
    transition-delay: 220ms;
  }

  .armed {
    animation: breathe 2.4s ease-in-out infinite;
  }
  .off {
    opacity: 0.55;
  }
  @keyframes breathe {
    50% {
      opacity: 0.5;
    }
  }
</style>
