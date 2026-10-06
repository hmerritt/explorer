import { useEffect, useRef } from 'react'
import { TRACKER, clamp, trackerLayout } from '../lib/scroll-tracker'
import type { SectionMeasurement } from '../lib/scroll-tracker'

type Section = { id: string; label: string }
const percentages = Array.from({ length: 101 }, (_, index) => index)
const digitValues = Array.from({ length: 10 }, (_, index) => index)

export function AnimatedSectionNavigator({
  sections,
}: {
  sections: Section[]
}) {
  const navigation = useRef<HTMLElement>(null)
  const ruler = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const nav = navigation.current!
    const rail = ruler.current!
    const groups = [...nav.querySelectorAll<HTMLElement>('.tracker-section')]
    const links = groups.map((group) =>
      group.querySelector<HTMLAnchorElement>('a')!,
    )
    const letters = groups.map((group) => [
      ...group.querySelectorAll<HTMLElement>('.tracker-letter'),
    ])
    const measurements = groups.map((group) => [
      ...group.querySelectorAll<HTMLElement>('.tracker-measure span'),
    ])
    const ticks = [...rail.querySelectorAll<HTMLElement>('.ruler-tick')]
    const tickLabels = [
      ...rail.querySelectorAll<HTMLElement>('.ruler-tick-label'),
    ]
    const tickTrack = rail.querySelector<HTMLElement>('.ruler-ticks')!
    const needle = rail.querySelector<HTMLElement>('.ruler-needle')!
    const readout = rail.querySelector<HTMLElement>('.ruler-readout')!
    const digits = [...rail.querySelectorAll<HTMLElement>('.ruler-digit-strip')]
    const topPool = nav.querySelector<HTMLElement>('.tracker-pool--top')!
    const bottomPool = nav.querySelector<HTMLElement>('.tracker-pool--bottom')!
    const desktop = window.matchMedia(
      '(min-width: 1024px) and (pointer: fine) and (hover: hover)',
    )
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)')
    let geometry: { section: SectionMeasurement; index: number }[] = []
    let documentHeight = 0
    let width = 0
    let height = 0
    let frame = 0
    let dirty = true
    let disposed = false

    function measure() {
      documentHeight = document.documentElement.scrollHeight
      width = window.innerWidth
      height = window.innerHeight
      geometry = sections.flatMap(({ id }, index) => {
        const element = document.getElementById(id)
        groups[index].hidden = !element
        if (!element) return []
        const rect = element.getBoundingClientRect()
        return [
          {
            index,
            section: {
              anchor: rect.top + window.scrollY,
              bottom: rect.bottom + window.scrollY,
              widths: measurements[index].map(
                (letter) => letter.getBoundingClientRect().width,
              ),
            },
          },
        ]
      })
      ticks.forEach((tick, index) => {
        tick.style.top = `${(documentHeight * index) / 100}px`
      })
      const widest = Math.max(
        260,
        ...geometry.map(({ section }) =>
          section.widths.reduce((sum, advance) => sum + advance, 0),
        ),
      )
      nav.style.setProperty(
        '--tracker-pool-width',
        `${widest + TRACKER.rail + 64}px`,
      )
      dirty = false
    }

    function paint() {
      frame = 0
      if (!desktop.matches) return
      if (dirty) measure()
      const layout = trackerLayout({
        sections: geometry.map(({ section }) => section),
        documentHeight,
        width,
        height,
        scrollY: window.scrollY,
      })
      nav.dataset.mode = reduced.matches ? 'static' : 'animated'
      rail.dataset.mode = reduced.matches ? 'static' : 'animated'
      layout.labels.forEach((label, position) => {
        const index = geometry[position].index
        const link = links[index]
        if (position === layout.activeIndex)
          link.setAttribute('aria-current', 'location')
        else link.removeAttribute('aria-current')
        groups[index].dataset.phase = label.phase
        if (!reduced.matches) {
          letters[index].forEach((element, letterIndex) => {
            const pose = label.letters[letterIndex]
            element.style.transform = `translate3d(${pose.x}px, ${pose.y}px, 0) translate(-50%, -50%) rotate(${pose.rotation}deg) scale(${pose.scale})`
            element.style.color = `color-mix(in srgb, #000 ${pose.ink * 100}%, var(--paper))`
          })
          const { left, right, top, bottom } = label.bounds
          link.style.left = `${left}px`
          link.style.top = `${top}px`
          link.style.width = `${right - left}px`
          link.style.height = `${bottom - top}px`
        }
      })
      topPool.style.height = `${TRACKER.header + TRACKER.topPad + (layout.activeIndex + 1) * TRACKER.slot + TRACKER.currentGap + TRACKER.heading + 64}px`
      bottomPool.style.height = `${(reduced.matches ? sections.length : layout.queued) * TRACKER.slot + TRACKER.bottomPad + 64}px`
      bottomPool.style.opacity = String(
        reduced.matches ? 1 : clamp(layout.queued),
      )
      const progress = layout.ruler
      tickTrack.style.transform = `translate3d(0, ${progress.tickOffset}px, 0)`
      tickLabels.forEach((element, index) => {
        const y = (documentHeight * index * 5) / 100 + progress.tickOffset
        let opacity = 0.5
        if (!reduced.matches) {
          layout.labels.forEach((label) => {
            const distance = Math.max(
              label.bounds.top - y,
              y - label.bounds.bottom,
              0,
            )
            if (distance < 24)
              opacity *=
                1 - label.detach * (1 - label.flip) * (1 - distance / 24)
          })
        }
        element.style.opacity = String(opacity)
      })
      needle.style.transform = `translate3d(0, ${progress.needle}px, 0)`
      needle.style.opacity = String(progress.needleOpacity)
      readout.dataset.above = String(progress.readoutAbove)
      String(progress.percent)
        .padStart(3, '0')
        .split('')
        .forEach((digit, index) => {
          digits[index].style.transform = `translateY(-${digit}em)`
        })
      rail.dataset.ready = 'true'
    }

    function schedule() {
      if (!frame && !disposed) frame = requestAnimationFrame(paint)
    }
    function refresh() {
      dirty = true
      schedule()
    }
    const observer = new ResizeObserver(refresh)
    observer.observe(document.body)
    window.addEventListener('scroll', schedule, { passive: true })
    window.addEventListener('resize', refresh)
    desktop.addEventListener('change', refresh)
    reduced.addEventListener('change', refresh)
    document.fonts.ready
      .then(() => {
        if (!disposed) refresh()
      })
      .catch(() => {})
    document.fonts.addEventListener('loadingdone', refresh)
    refresh()
    return () => {
      disposed = true
      cancelAnimationFrame(frame)
      observer.disconnect()
      window.removeEventListener('scroll', schedule)
      window.removeEventListener('resize', refresh)
      desktop.removeEventListener('change', refresh)
      reduced.removeEventListener('change', refresh)
      document.fonts.removeEventListener('loadingdone', refresh)
    }
  }, [sections])

  return (
    <>
      <nav
        className="section-navigation"
        aria-label="Page sections"
        ref={navigation}
        data-mode="static"
      >
        <div className="tracker-pool tracker-pool--top" aria-hidden="true" />
        <div className="tracker-pool tracker-pool--bottom" aria-hidden="true" />
        {sections.map(({ id, label }, index) => (
          <div className="tracker-section" key={id}>
            <a
              href={`#${id}`}
              className="tracker-link"
              aria-label={`Jump to ${label}`}
              aria-current={index === 0 ? 'location' : undefined}
            >
              <span className="tracker-static-label">{label}</span>
            </a>
            {[...label.toUpperCase()].map((letter, letterIndex) => (
              <span
                className="tracker-letter"
                aria-hidden="true"
                key={letterIndex}
              >
                {letter}
              </span>
            ))}
            <span className="tracker-measure" aria-hidden="true">
              {[...label.toUpperCase()].map((letter, letterIndex) => (
                <span key={letterIndex}>{letter}</span>
              ))}
            </span>
          </div>
        ))}
      </nav>
      <div
        className="scroll-ruler"
        ref={ruler}
        aria-hidden="true"
        data-ready="false"
        data-mode="static"
      >
        <div className="ruler-ticks">
          {percentages.map((percent) => (
            <div
              className={`ruler-tick ${percent % 5 === 0 ? 'ruler-tick--major' : ''}`}
              key={percent}
            >
              <i />
              {percent % 5 === 0 && (
                <span className="ruler-tick-label">{percent}</span>
              )}
            </div>
          ))}
        </div>
        <div className="ruler-needle">
          <i />
          <span className="ruler-readout">
            {[0, 1, 2].map((digit) => (
              <span className="ruler-digit" key={digit}>
                <span className="ruler-digit-strip">
                  {digitValues.map((value) => (
                    <span key={value}>{value}</span>
                  ))}
                </span>
              </span>
            ))}
            %
          </span>
        </div>
      </div>
    </>
  )
}
