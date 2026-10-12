import assert from 'node:assert/strict'
import test from 'node:test'
import { fitMedia } from '../src/lib/media-viewer.ts'

test('media fits desktop, portrait mobile and short landscape viewports without cropping', () => {
  for (const [vw, vh] of [
    [1440, 900],
    [390, 844],
    [844, 390],
  ]) {
    for (const [width, height] of [
      [1202, 822],
      [1026, 822],
      [1920, 1080],
      [600, 1600],
    ]) {
      const fitted = fitMedia(width, height, vw, vh)
      assert.ok(fitted.left > 0 && fitted.top > 0)
      assert.ok(fitted.left + fitted.width < vw)
      assert.ok(fitted.top + fitted.height < vh)
      assert.ok(Math.abs(fitted.width / fitted.height - width / height) < 1e-10)
      assert.equal(fitted.left, (vw - fitted.width) / 2)
      assert.equal(fitted.top, (vh - fitted.height) / 2)
    }
  }
})

test('wide and tall media use the available limiting dimension', () => {
  assert.equal(fitMedia(1920, 1080, 1440, 900).width, 1376)
  assert.equal(fitMedia(600, 1600, 1440, 900).height, 836)
})
