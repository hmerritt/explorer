import { createFileRoute } from '@tanstack/react-router'
import { useState } from 'react'

export const Route = createFileRoute('/')({ component: Home })

function Home() {
  const [count, setCount] = useState(0)

  return (
    <main>
      <h1>Hello, Explorer!</h1>
      <p>A cross-platform file explorer built with Rust and GPUI.</p>
      <button type="button" onClick={() => setCount((value) => value + 1)}>
        Count: {count}
      </button>
    </main>
  )
}
