import { useLayoutEffect } from 'react'
import { gsap } from 'gsap'
import { ScrollTrigger } from 'gsap/ScrollTrigger'
import { motionLifecycle, revealDelay, revealTween } from '../lib/page-motion'

const hidden = { autoAlpha: 0, y: 28, filter: 'blur(8px)' }

export function PageMotion() {
  useLayoutEffect(() => {
    const lifecycle = motionLifecycle()
    const releaseGate = () =>
      document.documentElement.removeAttribute('data-motion-pending')
    // A slow or failed hydration must not hide content again after fail-open.
    if (document.documentElement.hasAttribute('data-motion-expired')) {
      releaseGate()
      return
    }
    try {
      gsap.registerPlugin(ScrollTrigger)
      const context = gsap.context(() => {})
      lifecycle.add(() => context.revert())
      const media = gsap.matchMedia()
      lifecycle.add(() => media.revert())
      context.add(() => {
        media.add('(prefers-reduced-motion: no-preference)', () => {
          const run = motionLifecycle()
          // Also covers initialization throwing before matchMedia registers cleanup.
          lifecycle.add(() => run.dispose())
          const query = (selector: string) =>
            gsap.utils.toArray<HTMLElement>(selector)
          const opening = gsap.timeline()
          const nav = query('[data-nav-reveal]').filter(
            (element) => getComputedStyle(element).display !== 'none',
          )
          gsap.set(nav, { autoAlpha: 0, y: -6 })
          opening.to(
            nav,
            {
              autoAlpha: 1,
              y: 0,
              duration: 0.5,
              stagger: 0.08,
              ease: 'power2.out',
              clearProps: 'opacity,visibility,transform',
            },
            0.3,
          )
          const hero = query('[data-hero-reveal]')
          gsap.set(hero, { ...hidden })
          opening.to(hero, { ...revealTween(), stagger: 0.08 }, 0)
          const fastForward = () => opening.timeScale(4)
          const inputs = ['wheel', 'pointerdown', 'keydown', 'touchstart']
          inputs.forEach((name) =>
            window.addEventListener(name, fastForward, { passive: true }),
          )
          const removeInputs = () =>
            inputs.forEach((name) =>
              window.removeEventListener(name, fastForward),
            )
          opening.eventCallback('onComplete', removeInputs)
          run.add(removeInputs)

          query('[data-section-motion]').forEach((section) => {
            const rule = section.querySelector('[data-section-rule]')
            const ticks = section.querySelectorAll('[data-section-tick]')
            const copy = section.querySelector('[data-section-copy]')
            gsap.set(rule, { scaleX: 0 })
            gsap.set(ticks, { scale: 0, opacity: 0 })
            gsap.set(copy, { ...hidden })
            const timeline = gsap.timeline({
              scrollTrigger: { trigger: section, start: 'top 88%', once: true },
            })
            timeline.to(
              rule,
              {
                scaleX: 1,
                duration: 0.7,
                ease: 'power2.out',
                clearProps: 'transform',
              },
              0,
            )
            timeline.to(copy, revealTween(), 0.15)
            timeline.to(
              ticks,
              {
                scale: 1,
                opacity: 1,
                duration: 0.35,
                stagger: 0.06,
                ease: 'power3.out',
                clearProps: 'transform,opacity',
              },
              0.35,
            )
          })

          query('[data-reveal]').forEach((element) => {
            const group = element.closest('[data-reveal-group]')
            gsap.set(element, { ...hidden })
            ScrollTrigger.create({
              trigger: element,
              start: group ? 'top 88%' : 'top 85%',
              once: true,
              onEnter: () => {
                const bounds = group?.getBoundingClientRect()
                // Read at entry so responsive column changes update the stagger.
                const delay = bounds
                  ? revealDelay(
                      element.getBoundingClientRect().left,
                      bounds.left,
                      bounds.width,
                    )
                  : 0
                gsap.to(element, { ...revealTween(), delay })
              },
            })
          })

          let frame = 0
          let disposed = false
          const refresh = () => {
            if (disposed || frame) return
            frame = requestAnimationFrame(() => {
              frame = 0
              // Defer geometry refresh until active native scrolling finishes.
              ScrollTrigger.refresh(true)
            })
          }
          const observer = new ResizeObserver(refresh)
          observer.observe(document.body)
          document.addEventListener('load', refresh, true)
          document.fonts.addEventListener('loadingdone', refresh)
          void document.fonts.ready.then(refresh).catch(() => {})
          run.add(() => {
            disposed = true
            cancelAnimationFrame(frame)
            observer.disconnect()
            document.removeEventListener('load', refresh, true)
            document.fonts.removeEventListener('loadingdone', refresh)
          })
          refresh()
          return () => run.dispose()
        })
      })
    } catch (error) {
      lifecycle.dispose()
      console.error('Page entrance animations could not initialize', error)
    } finally {
      releaseGate()
    }
    return () => {
      lifecycle.dispose()
      releaseGate()
    }
  }, [])
  return null
}
