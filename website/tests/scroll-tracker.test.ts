import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  TRACKER,
  rulerGeometry,
  trackerLayout,
} from '../src/lib/scroll-tracker.ts'

const sections = [49, 1800, 2800, 4300, 6000].map((anchor, index, anchors) => ({
  anchor,
  bottom: anchors[index + 1] ?? 6250,
  widths: Array.from({ length: 8 }, () => 22),
}))
const layoutAt = (scrollY: number, height = 900) =>
  trackerLayout({
    sections,
    documentHeight: 6700,
    width: 1600,
    height,
    scrollY,
  })
const near = (actual: number, expected: number) =>
  assert.ok(Math.abs(actual - expected) < 0.000001, `${actual} ≠ ${expected}`)

test('the first section starts current at the top and future sections queue at the bottom', () => {
  const layout = layoutAt(0)
  assert.equal(layout.activeIndex, 0)
  assert.deepEqual(
    layout.labels.map((label) => label.phase),
    ['current', 'queued', 'queued', 'queued', 'queued'],
  )
  near(layout.labels[0].letters[0].scale, 1)
  near(layout.labels[4].letters[0].y, 900 - TRACKER.bottomPad)
  near(layout.labels[3].letters[0].y, 900 - TRACKER.bottomPad - TRACKER.slot)
})

test('a section detaches with staggered letters, rides vertically, then enters the top stack', () => {
  const detaching = layoutAt(900).labels[1]
  assert.ok(detaching.detach > 0 && detaching.detach < 1)
  assert.equal(detaching.flip, 0)
  assert.notEqual(detaching.letters[0].rotation, detaching.letters[7].rotation)
  const riding = layoutAt(1250).labels[1]
  assert.equal(riding.detach, 1)
  assert.equal(riding.flip, 0)
  assert.ok(riding.letters.every((letter) => letter.rotation === -90))
  const entering = layoutAt(1600).labels[1]
  assert.ok(entering.flip > 0 && entering.flip < 1)
  assert.equal(entering.phase, 'entering')
  assert.ok(layoutAt(1600).labels[0].letters[0].scale < 1)
  const completed = layoutAt(1850)
  assert.equal(completed.activeIndex, 1)
  assert.equal(completed.labels[0].phase, 'passed')
  assert.equal(completed.labels[1].phase, 'current')
  assert.ok(
    completed.labels[1].letters.every(
      (letter) => letter.rotation === 0 && letter.scale === 1,
    ),
  )
})

test('both transitions use twice the previous rail travel distance', () => {
  const detachRails: number[] = []
  const flipRails: number[] = []
  for (let scroll = 0; scroll <= 2000; scroll++) {
    const layout = layoutAt(scroll)
    const label = layout.labels[1]
    const railY = sections[1].anchor - scroll + layout.ruler.drift
    if (label.detach > 0 && label.detach < 1) detachRails.push(railY)
    if (label.flip > 0 && label.flip < 1) flipRails.push(railY)
  }
  const travel = (rails: number[]) => rails[0] - rails.at(-1)!
  assert.ok(travel(detachRails) > 475 && travel(detachRails) <= 480)
  assert.ok(travel(flipRails) > 395 && travel(flipRails) <= 400)
  for (const scroll of [800, 1000, 1450, 1650]) {
    assert.ok(
      layoutAt(scroll).labels[1].letters.some(
        (letter) => letter.rotation < 0 && letter.rotation > -90,
      ),
    )
  }
})

test('short viewports and long labels finish detaching before a visible vertical ride and top entry', () => {
  for (const height of [360, 600, 900]) {
    for (const length of [8, 20]) {
      const measured = sections.map((section) => ({
        ...section,
        widths: Array.from({ length }, () => 22),
      }))
      const ridingRails: number[] = []
      let sawDetachment = false
      let sawEntry = false
      for (let scrollY = 0; scrollY <= 2100; scrollY++) {
        const layout = trackerLayout({
          sections: measured,
          documentHeight: 6700,
          width: 1100,
          height,
          scrollY,
        })
        const label = layout.labels[1]
        if (label.detach > 0 && label.detach < 1) sawDetachment = true
        if (label.detach === 1 && label.flip === 0) {
          assert.ok(label.letters.every((letter) => letter.rotation === -90))
          ridingRails.push(measured[1].anchor - scrollY + layout.ruler.drift)
        }
        if (label.flip > 0) {
          assert.equal(label.detach, 1)
          sawEntry = true
        }
      }
      assert.ok(sawDetachment && sawEntry)
      assert.ok(ridingRails[0] - ridingRails.at(-1)! >= TRACKER.ridingGap - 2)
    }
  }
})

test('the final-section fallback spans at least 48px when space permits', () => {
  const measured = [
    sections[0],
    { anchor: 1700, bottom: 2000, widths: sections[1].widths },
  ]
  const at = (scrollY: number) =>
    trackerLayout({
      sections: measured,
      documentHeight: 2000,
      width: 1600,
      height: 900,
      scrollY,
    })
  assert.equal(at(1052).labels[1].flip, 0)
  near(at(1076).labels[1].flip, 0.5)
  assert.equal(at(1076).labels[1].detach, 1)
  assert.ok(
    at(1076).labels[1].letters.some((letter) => letter.rotation === -90),
  )
  assert.equal(at(1100).activeIndex, 1)
  assert.ok(at(1100).labels[1].letters.every((letter) => letter.rotation === 0))
})

test('reverse scrolling and large jumps produce deterministic positions without remembered state', () => {
  const before = layoutAt(1600)
  layoutAt(5000)
  assert.deepEqual(layoutAt(1600), before)
  assert.equal(layoutAt(0).activeIndex, 0)
  assert.equal(layoutAt(4500).activeIndex, 3)
})

test('the document percentage tick stays aligned with the progress needle', () => {
  for (const scroll of [0, 100, 1600, 2900, 5800]) {
    const ruler = rulerGeometry(6700, 900, scroll)
    near(ruler.progress * 6700 + ruler.tickOffset, ruler.needle)
    assert.ok(ruler.needle >= 61 && ruler.needle <= 888)
  }
  const half = rulerGeometry(6700, 900, 2900)
  assert.equal(half.percent, 50)
  near(half.needle, (61 + 888) / 2)
})

test('progress clamps overscroll, fades in over 3%, and keeps the bottom readout on screen', () => {
  assert.equal(rulerGeometry(6700, 900, -100).percent, 0)
  assert.equal(rulerGeometry(6700, 900, 99999).percent, 100)
  assert.equal(rulerGeometry(6700, 900, 0).needleOpacity, 0)
  near(rulerGeometry(6700, 900, 87).needleOpacity, 0.5)
  assert.equal(rulerGeometry(6700, 900, 174).needleOpacity, 1)
  assert.equal(rulerGeometry(6700, 900, 5800).readoutAbove, true)
})

test('a short final section reaches the top stack at the bottom of the document', () => {
  const bottom = layoutAt(5800)
  assert.equal(bottom.activeIndex, 4)
  assert.equal(bottom.labels[4].phase, 'current')
  assert.ok(bottom.labels[4].letters.every((letter) => letter.rotation === 0))
  assert.equal(layoutAt(5000).labels[4].flip, 0)
})

test('short documents, empty sections, and short viewports remain finite', () => {
  const ruler = rulerGeometry(500, 900, 100)
  assert.equal(ruler.progress, 0)
  assert.equal(ruler.scroll, 0)
  assert.ok(Number.isFinite(ruler.needle))
  const short = trackerLayout({
    sections: sections.slice(0, 2).map((section, index) => ({
      ...section,
      anchor: index * 200,
      bottom: (index + 1) * 200,
    })),
    documentHeight: 500,
    width: 1600,
    height: 900,
    scrollY: 100,
  })
  assert.equal(short.activeIndex, 0)
  assert.deepEqual(
    short.labels.map((label) => label.phase),
    ['current', 'queued'],
  )
  assert.equal(
    trackerLayout({
      sections: [],
      documentHeight: 0,
      width: 1600,
      height: 900,
      scrollY: 0,
    }).activeIndex,
    -1,
  )
  for (const height of [360, 600]) {
    const frame = layoutAt(6700 - height, height)
    assert.equal(frame.activeIndex, 4)
    frame.labels.forEach((label) =>
      Object.values(label.bounds).forEach((value) =>
        assert.ok(Number.isFinite(value)),
      ),
    )
  }
})

test('hit bounds enclose rotated characters throughout both transitions', () => {
  for (const scroll of [0, 1100, 1400, 1600, 1850, 4500, 5800]) {
    layoutAt(scroll).labels.forEach((label) => {
      label.letters.forEach((letter) => {
        assert.ok(
          letter.x >= label.bounds.left && letter.x <= label.bounds.right,
        )
        assert.ok(
          letter.y >= label.bounds.top && letter.y <= label.bounds.bottom,
        )
      })
      assert.ok(label.bounds.right > label.bounds.left)
      assert.ok(label.bounds.bottom > label.bounds.top)
    })
  }
})
