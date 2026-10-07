<script>
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";

  // on: built-in display on · armed: waiting for an external monitor · off: built-in display off
  let display = $state("on");
  const lidAngle = { on: 100, armed: 40, off: 1.5 };

  invoke("get_state").then((s) => (display = s));
  listen("state", (event) => (display = event.payload));

  function onkeydown(event) {
    if (event.key === "Escape") invoke("hide");
  }
</script>

<svelte:window {onkeydown} />

<button class={display} onclick={() => invoke("toggle_display")} aria-label="Toggle built-in display">
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
  }

  button {
    all: unset;
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    color: var(--ink);
  }
  button:focus-visible svg {
    outline: 1.5px solid color-mix(in srgb, var(--ink) 35%, transparent);
    outline-offset: 6px;
    border-radius: 12px;
  }

  svg {
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
