/** PhotoCraft's horizontal group stagger, in seconds. */
export function revealDelay(
  left: number,
  groupLeft: number,
  groupWidth: number,
) {
  return groupWidth > 0 ? ((left - groupLeft) / groupWidth) * 0.18 : 0
}

// GSAP mutates tween vars (including their parent timeline). Never share them.
export function revealTween() {
  return {
    autoAlpha: 1,
    y: 0,
    filter: 'blur(0px)',
    duration: 0.9,
    ease: 'power3.out',
    overwrite: true,
    clearProps: 'opacity,visibility,transform,filter',
  }
}

// Runs before paint. Content stays visible without JS or if hydration fails.
export const motionBootstrap = `(() => {
  try {
    if (location.pathname !== '/' || matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    document.documentElement.setAttribute('data-motion-pending', '');
    setTimeout(() => {
      if (!document.documentElement.hasAttribute('data-motion-pending')) return;
      document.documentElement.removeAttribute('data-motion-pending');
      document.documentElement.setAttribute('data-motion-expired', '');
    }, 3000);
  } catch {}
})()`

/** Own resources in reverse creation order, including partially initialized runs. */
export function motionLifecycle() {
  const resources: (() => void)[] = []
  let disposed = false
  return {
    add(cleanup: () => void) {
      if (disposed) cleanup()
      else resources.push(cleanup)
    },
    dispose() {
      if (disposed) return
      disposed = true
      for (const cleanup of resources.reverse()) cleanup()
    },
  }
}
