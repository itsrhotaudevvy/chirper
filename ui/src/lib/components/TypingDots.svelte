<script lang="ts">
  interface Props {
    /** Dot colour. Defaults to the muted text colour. */
    color?: string;
    /** Accessible label, for example "Sam is typing". */
    label?: string;
  }

  let { color = 'var(--on-muted)', label = 'Typing' }: Props = $props();
</script>

<span class="dots" role="status" aria-label={label} style:--dot={color}>
  <span></span><span></span><span></span>
</span>

<style>
  .dots {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 16px;
  }

  .dots span {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dot);
    animation: bob 1.2s var(--ease-out) infinite;
  }

  .dots span:nth-child(2) {
    animation-delay: 0.16s;
  }

  .dots span:nth-child(3) {
    animation-delay: 0.32s;
  }

  @keyframes bob {
    0%,
    60%,
    100% {
      transform: translateY(0);
      opacity: 0.45;
    }
    30% {
      transform: translateY(-4px);
      opacity: 1;
    }
  }
</style>
