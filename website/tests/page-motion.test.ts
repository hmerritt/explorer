import { test } from 'node:test'
import assert from 'node:assert/strict'
import { runInNewContext } from 'node:vm'
import { gsap } from 'gsap'
import {
  motionBootstrap,
  motionLifecycle,
  revealDelay,
  revealTween,
} from '../src/lib/page-motion.ts'

test('section timelines cannot capture later standalone reveals through shared tween vars', () => {
  const timeline = gsap.timeline({ paused: true })
  const vars = revealTween()
  timeline.to({ value: 0 }, vars, 0.15)
  // GSAP adds its parent to the exact vars object it receives.
  assert.equal(Reflect.get(vars, 'parent'), timeline)
  assert.equal(Reflect.get(revealTween(), 'parent'), undefined)
  assert.notEqual(revealTween(), revealTween())
  timeline.kill()
})

test('group delay follows horizontal position, including responsive columns', () => {
  assert.equal(revealDelay(100, 100, 1200), 0)
  assert.equal(revealDelay(500, 100, 1200), 0.06)
  assert.equal(revealDelay(900, 100, 1200), 0.12)
  assert.equal(revealDelay(700, 100, 1200), 0.09)
  assert.equal(revealDelay(100, 100, 390), 0)
  assert.equal(revealDelay(100, 100, 0), 0)
})

test('partial initialization and teardown release resources once in reverse order', () => {
  const lifecycle = motionLifecycle()
  const released: number[] = []
  lifecycle.add(() => released.push(1))
  lifecycle.add(() => released.push(2))
  lifecycle.dispose()
  lifecycle.dispose()
  lifecycle.add(() => released.push(3))
  assert.deepEqual(released, [2, 1, 3])
})

function bootstrap(reduced = false, pathname = '/') {
  const attributes = new Set<string>()
  let timeout: (() => void) | undefined
  runInNewContext(motionBootstrap, {
    location: { pathname },
    matchMedia: () => ({ matches: reduced }),
    document: {
      documentElement: {
        setAttribute: (name: string) => attributes.add(name),
        removeAttribute: (name: string) => attributes.delete(name),
        hasAttribute: (name: string) => attributes.has(name),
      },
    },
    setTimeout: (callback: () => void, duration: number) => {
      assert.equal(duration, 3000)
      timeout = callback
    },
  })
  return { attributes, timeout }
}

test('prepaint gate fails open after three seconds without hydration', () => {
  const { attributes, timeout } = bootstrap()
  assert.ok(attributes.has('data-motion-pending'))
  assert.ok(timeout)
  timeout()
  assert.ok(!attributes.has('data-motion-pending'))
  assert.ok(attributes.has('data-motion-expired'))
})

test('successful initialization releases the gate without expiring later', () => {
  const { attributes, timeout } = bootstrap()
  attributes.delete('data-motion-pending')
  timeout!()
  assert.equal(attributes.size, 0)
})

test('reduced motion and other routes never hide prerendered content', () => {
  for (const result of [bootstrap(true), bootstrap(false, '/missing')]) {
    assert.equal(result.attributes.size, 0)
    assert.equal(result.timeout, undefined)
  }
  assert.doesNotThrow(() => runInNewContext(motionBootstrap, {}))
})
