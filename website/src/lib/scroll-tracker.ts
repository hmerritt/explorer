export const TRACKER = {
  header: 49,
  edge: 12,
  rail: 60,
  heading: 34,
  riding: 20,
  compact: 16,
  topPad: 28,
  currentGap: 26,
  slot: 20,
  bottomPad: 36,
  flipZone: 200,
  detachZone: 240,
  stagger: 0.16,
  arc: 28,
  muted: 0.35,
} as const

export const clamp = (value: number) => Math.max(0, Math.min(1, value))
const mix = (from: number, to: number, progress: number) =>
  from + (to - from) * progress
const ease = (value: number) =>
  value < 0.5 ? 4 * value ** 3 : 1 - (-2 * value + 2) ** 3 / 2

export function rulerGeometry(
  documentHeight: number,
  viewportHeight: number,
  scrollY: number,
) {
  const maxScroll = Math.max(0, documentHeight - viewportHeight)
  const scroll = Math.max(0, Math.min(maxScroll, scrollY))
  const progress = maxScroll > 0 ? scroll / maxScroll : 0
  const start = Math.min(TRACKER.header + TRACKER.edge, viewportHeight)
  const span = Math.max(0, viewportHeight - TRACKER.edge - start)
  const drift = start + progress * (span - viewportHeight)
  const needle = start + progress * span
  return {
    maxScroll,
    scroll,
    progress,
    drift,
    needle,
    tickOffset: drift - scroll,
    needleOpacity: clamp(progress / 0.03),
    readoutAbove: needle > viewportHeight - 26,
    percent: Math.round(progress * 100),
  }
}

export type SectionMeasurement = {
  anchor: number
  bottom: number
  /** Individual character advances measured at the heading font size. */
  widths: number[]
}

type LetterPose = {
  x: number
  y: number
  rotation: number
  scale: number
  ink: number
}

function interpolate(from: LetterPose, to: LetterPose, progress: number) {
  const eased = ease(clamp(progress))
  return {
    x: mix(from.x, to.x, eased) - 2 * TRACKER.arc * eased * (1 - eased),
    y: mix(from.y, to.y, eased),
    rotation: mix(from.rotation, to.rotation, eased),
    scale: mix(from.scale, to.scale, eased),
    ink: mix(from.ink, to.ink, eased),
  }
}

export function trackerLayout({
  sections,
  documentHeight,
  width,
  height,
  scrollY,
}: {
  sections: SectionMeasurement[]
  documentHeight: number
  width: number
  height: number
  scrollY: number
}) {
  const ruler = rulerGeometry(documentHeight, height, scrollY)
  const states = sections.map((section, index) => {
    const length =
      section.widths.reduce((sum, advance) => sum + advance, 0) *
      (TRACKER.riding / TRACKER.heading)
    const railY = section.anchor - ruler.scroll + ruler.drift
    let flip =
      index === 0
        ? 1
        : ruler.maxScroll === 0
          ? 0
          : clamp((height * 0.12 + TRACKER.flipZone - railY) / TRACKER.flipZone)
    // A short final section may never reach the flip line naturally.
    if (
      index > 0 &&
      ruler.maxScroll > 0 &&
      section.anchor - ruler.maxScroll > height * 0.12
    ) {
      const start = Math.max(
        0,
        Math.min(section.bottom - height, ruler.maxScroll - 24),
      )
      flip = Math.max(
        flip,
        clamp((ruler.scroll - start) / (ruler.maxScroll - start)),
      )
    }
    return { railY, length, flip, detach: index === 0 ? 1 : 0, queueY: 0 }
  })
  let queued = 0
  for (let index = states.length - 1; index > 0; index--) {
    const state = states[index]
    state.queueY = height - TRACKER.bottomPad - TRACKER.slot * queued
    state.detach = clamp(
      (state.queueY + TRACKER.detachZone - state.railY - state.length) /
        TRACKER.detachZone,
    )
    if (ruler.maxScroll === 0) state.detach = 0
    queued += (1 - state.detach) * (1 - state.flip)
  }
  let activeIndex = -1
  states.forEach((state, index) => {
    if (state.flip === 1) activeIndex = index
  })
  const entering = clamp(
    states.reduce((sum, state) => sum + (state.flip < 1 ? state.flip : 0), 0),
  )
  const labels = sections.map((section, index) => {
    const state = states[index]
    const total = section.widths.reduce((sum, advance) => sum + advance, 0)
    let advance = 0
    const shrink = index === activeIndex ? entering : 1
    const topY =
      TRACKER.header +
      TRACKER.topPad +
      index * TRACKER.slot +
      TRACKER.currentGap * (state.flip < 1 ? 1 : 1 - shrink)
    const topScale =
      state.flip < 1 ? 1 : mix(1, TRACKER.compact / TRACKER.heading, shrink)
    const topInk = state.flip < 1 ? 1 : mix(1, TRACKER.muted, shrink)
    const letters = section.widths.map((letterWidth, letterIndex) => {
      const center = advance + letterWidth / 2
      advance += letterWidth
      const horizontal = (
        y: number,
        scale: number,
        ink: number,
      ): LetterPose => ({
        x: width - TRACKER.rail - (total - center) * scale,
        y,
        rotation: 0,
        scale,
        ink,
      })
      const vertical = (y: number): LetterPose => ({
        x: width - TRACKER.rail / 2,
        y: y + (total - center) * (TRACKER.riding / TRACKER.heading),
        rotation: -90,
        scale: TRACKER.riding / TRACKER.heading,
        ink: TRACKER.muted,
      })
      const staggered = (progress: number) =>
        progress * (1 + (section.widths.length - 1) * TRACKER.stagger) -
        (section.widths.length - 1 - letterIndex) * TRACKER.stagger
      if (state.flip === 1) return horizontal(topY, topScale, topInk)
      if (state.flip > 0) {
        return interpolate(
          vertical(state.railY),
          horizontal(topY, 1, 1),
          staggered(state.flip),
        )
      }
      if (state.detach < 1) {
        return interpolate(
          horizontal(
            state.queueY,
            TRACKER.compact / TRACKER.heading,
            TRACKER.muted,
          ),
          vertical(state.queueY - state.length),
          staggered(state.detach),
        )
      }
      return vertical(state.railY)
    })
    const bounds = {
      left: Infinity,
      top: Infinity,
      right: -Infinity,
      bottom: -Infinity,
    }
    letters.forEach((letter, letterIndex) => {
      const radians = (letter.rotation * Math.PI) / 180
      const w = section.widths[letterIndex] * letter.scale
      const h = TRACKER.heading * letter.scale
      const halfWidth =
        (Math.abs(w * Math.cos(radians)) + Math.abs(h * Math.sin(radians))) / 2
      const halfHeight =
        (Math.abs(w * Math.sin(radians)) + Math.abs(h * Math.cos(radians))) / 2
      bounds.left = Math.min(bounds.left, letter.x - halfWidth - 4)
      bounds.right = Math.max(bounds.right, letter.x + halfWidth + 4)
      bounds.top = Math.min(bounds.top, letter.y - halfHeight - 4)
      bounds.bottom = Math.max(bounds.bottom, letter.y + halfHeight + 4)
    })
    return {
      letters,
      bounds,
      flip: state.flip,
      detach: state.detach,
      phase:
        state.flip === 1
          ? index === activeIndex
            ? 'current'
            : 'passed'
          : state.flip > 0
            ? 'entering'
            : state.detach === 0
              ? 'queued'
              : 'riding',
    }
  })
  return { ruler, labels, activeIndex, queued }
}
