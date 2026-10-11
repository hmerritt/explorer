import { test } from 'node:test'
import assert from 'node:assert/strict'
import { siteOrigin } from '../src/lib/site-url.ts'

test('defaults to the public site and normalizes configured origins', () => {
  for (const value of [undefined, '', '   ']) {
    assert.equal(siteOrigin(value), 'http://explorer.mrrtt.me')
    assert.equal(siteOrigin(value, true), 'http://explorer.mrrtt.me')
  }
  assert.equal(
    siteOrigin('http://explorer.mrrtt.me/', true),
    'http://explorer.mrrtt.me',
  )
  assert.equal(
    siteOrigin(' HTTPS://Explorer.mrrtt.me/// ', true),
    'https://explorer.mrrtt.me',
  )
  assert.equal(siteOrigin('http://localhost:3000/'), 'http://localhost:3000')
})

test('production requires a public domain for configured origins', () => {
  for (const value of [
    'https://localhost',
    'https://app.localhost',
    'https://127.0.0.1',
    'https://[::1]',
    'https://app.local',
    'https://app.test',
    'https://a&b.com',
    'https://app.home.arpa',
  ]) {
    assert.throws(() => siteOrigin(value, true), /VITE_SITE_URL/)
  }
  assert.equal(
    siteOrigin('https://explorer.mrrtt.me/', true),
    'https://explorer.mrrtt.me',
  )
})

test('rejects malformed origins and URL components that would corrupt metadata', () => {
  for (const value of [
    'not a URL',
    'ftp://explorer.mrrtt.me',
    'https://user:password@explorer.mrrtt.me',
    'https://explorer.mrrtt.me/path',
    'https://explorer.mrrtt.me/?q=1',
    'https://explorer.mrrtt.me/#home',
  ]) {
    assert.throws(() => siteOrigin(value), /VITE_SITE_URL/)
  }
})
