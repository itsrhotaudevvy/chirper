<script lang="ts">
  import { haptic } from '../haptics';

  interface Props {
    checked?: boolean;
    /** Required: the accessible name and visible row label. */
    label: string;
    /** Secondary line under the label. */
    description?: string;
    disabled?: boolean;
    /** Track colour when on (CSS colour or network id). Defaults to the accent. */
    color?: string;
    onchange?: (checked: boolean) => void;
  }

  let { checked = $bindable(false), label, description, disabled = false, color, onchange }: Props = $props();

  function toggle(): void {
    checked = !checked;
    haptic('selection');
    onchange?.(checked);
  }
</script>

<button
  class="row"
  type="button"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  onclick={toggle}
  style:--toggle-on={color ?? 'var(--accent)'}
>
  <span class="text">
    <span class="label t-headline">{label}</span>
    {#if description}<span class="desc t-body t-muted">{description}</span>{/if}
  </span>
  <span class="track" class:on={checked} aria-hidden="true"><span class="thumb"></span></span>
</button>

<style>
  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-4);
    width: 100%;
    min-height: var(--hit);
    padding: var(--s-3) var(--gutter);
    text-align: start;
  }

  .row:disabled {
    opacity: 0.45;
  }

  .text {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 2px;
    min-width: 0;
  }

  .desc {
    font-size: var(--t-label-size);
  }

  .track {
    position: relative;
    flex: none;
    width: 51px;
    height: 31px;
    border-radius: var(--r-pill);
    background: var(--outline);
    transition: background-color var(--dur-base) var(--ease-out);
  }

  .track.on {
    background: var(--toggle-on);
  }

  .thumb {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 27px;
    height: 27px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: var(--elev-1);
    transition: transform var(--spring-dur) var(--spring);
  }

  .track.on .thumb {
    transform: translateX(20px);
  }
</style>
