<script lang="ts">
  import Icon from './Icon.svelte';
  import type { IconName, IconSize } from './icons';
  import { haptic } from '../haptics';

  interface Props {
    icon: IconName;
    /** Required: the accessible name of the button. */
    label: string;
    onclick?: (event: MouseEvent) => void;
    disabled?: boolean;
    /** Icon colour. Defaults to inherited text colour. */
    color?: string;
    /** Icon size in px (20 or 24); the touch target stays 44 px. */
    size?: IconSize;
    /** Tints the button background with the colour (for example a send button). */
    filled?: boolean;
    /** Plays a light haptic on press. */
    tap?: boolean;
    /** A toggle: shows its state as aria-pressed and a light disc behind the icon. */
    pressed?: boolean;
  }

  let { icon, label, onclick, disabled = false, color, size = 24, filled = false, tap = false, pressed }: Props = $props();

  function press(event: MouseEvent): void {
    if (tap) haptic('light');
    onclick?.(event);
  }
</script>

<button
  class="icon-button"
  class:filled
  class:pressed={pressed === true}
  type="button"
  aria-label={label}
  aria-pressed={pressed}
  title={label}
  {disabled}
  onclick={press}
  style:--ctx={color ?? 'currentColor'}
>
  <Icon name={icon} {size} />
</button>

<style>
  .icon-button {
    display: inline-grid;
    place-items: center;
    width: var(--hit);
    height: var(--hit);
    min-width: var(--hit);
    border-radius: 50%;
    color: var(--ctx);
    transition:
      transform var(--dur-quick) var(--ease-out),
      background-color var(--dur-base) var(--ease-out);
  }


  @media (hover: hover) and (pointer: fine) {
    .icon-button:hover:not(:disabled) {
      background: var(--surface-high);
    }
  }

  .icon-button:active:not(:disabled) {
    transform: scale(0.92);
  }

  .icon-button:disabled {
    opacity: 0.4;
  }

  .icon-button.pressed {
    background: rgb(255 255 255 / 0.55);
  }

  .icon-button.filled {
    background: var(--ctx);
    color: var(--on-accent);
  }
</style>
