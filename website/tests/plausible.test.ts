import { test } from 'node:test'
import assert from 'node:assert/strict'
import { plausibleScriptUrl } from '../src/lib/plausible.ts'

const production = {
  command: 'build',
  mode: 'production',
  nodeEnv: 'production',
}
const script = 'https://analytics.example.com/js/script.js'

test('missing, blank and invalid analytics URLs silently disable analytics', () => {
  for (const value of [
    undefined,
    '',
    '   ',
    'not a URL',
    '/js/script.js',
    '//analytics.example.com/js/script.js',
    'https://',
    'https:analytics.example.com/script.js',
    'ftp://analytics.example.com/script.js',
    'javascript:alert(1)',
    'data:text/javascript,alert(1)',
    'https://user:password@analytics.example.com/script.js',
    'https://user@analytics.example.com/script.js',
  ])
    assert.equal(plausibleScriptUrl(value, production), undefined)
})

test('accepts HTTP(S) script URLs and trims surrounding whitespace', () => {
  assert.equal(plausibleScriptUrl(`  ${script}  `, production), script)
  const http = 'http://analytics.example.com:8080/js/script.js?outbound=1'
  assert.equal(plausibleScriptUrl(http, production), http)
  assert.equal(
    plausibleScriptUrl(script, { ...production, nodeEnv: undefined }),
    script,
  )
})

test('development servers and development/test environments omit analytics', () => {
  assert.equal(
    plausibleScriptUrl(script, { ...production, command: 'serve' }),
    undefined,
  )
  for (const name of ['development', 'test']) {
    assert.equal(
      plausibleScriptUrl(script, { ...production, mode: name }),
      undefined,
    )
    assert.equal(
      plausibleScriptUrl(script, { ...production, nodeEnv: name }),
      undefined,
    )
  }
})
