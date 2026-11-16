<script lang="ts">
  interface Props {
    width?: string;
    height?: string;
    radius?: string;
  }

  let { width = '100%', height = '16px', radius = 'var(--r-xs)' }: Props = $props();
</script>

<span class="skeleton" aria-hidden="true" style:width style:height style:border-radius={radius}></span>

<style>
  .skeleton {
    display: block;
    background: linear-gradient(90deg, var(--surface-high) 25%, var(--outline) 50%, var(--surface-high) 75%);
    background-size: 200% 100%;
    animation: shimmer 1.4s linear infinite;
  }

  @keyframes shimmer {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: -100% 0;
    }
  }
</style>
