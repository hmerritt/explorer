import { useEffect, useRef, useState } from 'react'
import type { MouseEvent, ReactNode } from 'react'
import { fitMedia } from '../lib/media-viewer'

type PreviewMedia = HTMLImageElement | HTMLVideoElement
type Session = {
  trigger: HTMLElement
  preview: PreviewMedia
  playing: boolean
  time: number
}

export function MediaPreview({
  src,
  label,
  video = false,
  className,
  children,
}: {
  src: string
  label: string
  video?: boolean
  className?: string
  children: ReactNode
}) {
  const [session, setSession] = useState<Session | null>(null)
  const props = {
    className: `media-preview ${className ?? ''}`,
    'aria-label': label,
    'aria-haspopup': 'dialog' as const,
    onClick: (event: MouseEvent<HTMLElement>) => {
      if (
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
      )
        return
      const preview =
        event.currentTarget.querySelector<PreviewMedia>('img, video')
      if (!preview) return
      event.preventDefault()
      const playing = preview instanceof HTMLVideoElement && !preview.paused
      const time = preview instanceof HTMLVideoElement ? preview.currentTime : 0
      if (preview instanceof HTMLVideoElement) preview.pause()
      setSession({ trigger: event.currentTarget, preview, playing, time })
    },
  }
  return (
    <>
      {video ? (
        <button {...props} type="button">
          {children}
        </button>
      ) : (
        <a {...props} href={src}>
          {children}
        </a>
      )}
      {session && (
        <MediaViewer
          src={src}
          label={label}
          video={video}
          session={session}
          onClose={() => setSession(null)}
        />
      )}
    </>
  )
}

function MediaViewer({
  src,
  label,
  video,
  session,
  onClose,
}: {
  src: string
  label: string
  video: boolean
  session: Session
  onClose: () => void
}) {
  const dialog = useRef<HTMLDialogElement>(null)
  const stage = useRef<HTMLDivElement>(null)
  const dismiss = useRef<() => void>(() => undefined)
  const closeCallback = useRef(onClose)
  useEffect(() => {
    closeCallback.current = onClose
  }, [onClose])

  useEffect(() => {
    const modal = dialog.current!
    const frame = stage.current!
    const preview = session.preview
    const reduced = window.matchMedia(
      '(prefers-reduced-motion: reduce)',
    ).matches
    const duration = reduced ? 0 : 200
    let closing = false
    let animation: Animation | undefined
    let backdropAnimation: Animation | undefined
    let timer: ReturnType<typeof setTimeout> | undefined
    const root = document.documentElement
    const previousOverflow = root.style.overflow
    const previousBodyOverflow = document.body.style.overflow
    root.style.overflow = 'hidden'
    document.body.style.overflow = 'hidden'
    modal.showModal()
    if (preview instanceof HTMLVideoElement) preview.pause()

    const layout = () => {
      const width =
        preview instanceof HTMLVideoElement
          ? preview.videoWidth
          : preview.naturalWidth
      const height =
        preview instanceof HTMLVideoElement
          ? preview.videoHeight
          : preview.naturalHeight
      const target = fitMedia(
        width || preview.clientWidth,
        height || preview.clientHeight,
        window.innerWidth,
        window.innerHeight,
      )
      Object.assign(frame.style, {
        width: `${target.width}px`,
        height: `${target.height}px`,
        left: `${target.left}px`,
        top: `${target.top}px`,
      })
    }
    const origin = () => {
      const from = preview.getBoundingClientRect()
      const to = {
        left: frame.offsetLeft,
        top: frame.offsetTop,
        width: frame.offsetWidth,
        height: frame.offsetHeight,
      }
      return `translate(${from.left - to.left}px, ${from.top - to.top}px) scale(${from.width / to.width}, ${from.height / to.height})`
    }
    layout()
    animation = frame.animate(
      [{ transform: origin() }, { transform: 'none' }],
      { duration, easing: 'ease-out' },
    )
    backdropAnimation = modal.animate(
      [
        { backgroundColor: 'rgba(0,0,0,0)' },
        { backgroundColor: 'rgba(0,0,0,0.85)' },
      ],
      { duration },
    )

    const close = () => {
      if (closing) return
      closing = true
      // Keep the modal blocking the page until the return animation finishes.
      const currentTransform = getComputedStyle(frame).transform
      const currentBackdrop = getComputedStyle(modal).backgroundColor
      animation?.cancel()
      backdropAnimation?.cancel()
      animation = frame.animate(
        [{ transform: currentTransform }, { transform: origin() }],
        { duration, easing: 'ease-in', fill: 'forwards' },
      )
      backdropAnimation = modal.animate(
        [
          { backgroundColor: currentBackdrop },
          { backgroundColor: 'rgba(0,0,0,0)' },
        ],
        { duration, fill: 'forwards' },
      )
      timer = setTimeout(() => closeCallback.current(), duration)
    }
    dismiss.current = close
    const consume = (event: Event) => {
      event.preventDefault()
      event.stopPropagation()
      close()
    }
    const keyboard = (event: KeyboardEvent) => {
      if (event.key === 'Tab') {
        event.preventDefault()
        modal.querySelector<HTMLButtonElement>('button')?.focus()
        return
      }
      if (
        [
          'Escape',
          'ArrowUp',
          'ArrowDown',
          'PageUp',
          'PageDown',
          'Home',
          'End',
          ' ',
        ].includes(event.key)
      )
        consume(event)
    }
    modal.addEventListener('click', consume)
    modal.addEventListener('wheel', consume, { passive: false })
    modal.addEventListener('touchmove', consume, { passive: false })
    modal.addEventListener('keydown', keyboard)
    window.addEventListener('resize', layout)
    return () => {
      clearTimeout(timer)
      animation?.cancel()
      backdropAnimation?.cancel()
      modal.querySelector('video')?.pause()
      modal.removeEventListener('click', consume)
      modal.removeEventListener('wheel', consume)
      modal.removeEventListener('touchmove', consume)
      modal.removeEventListener('keydown', keyboard)
      window.removeEventListener('resize', layout)
      modal.close()
      root.style.overflow = previousOverflow
      document.body.style.overflow = previousBodyOverflow
      session.trigger.focus({ preventScroll: true })
      if (preview instanceof HTMLVideoElement && session.playing)
        void preview.play().catch(() => undefined)
    }
  }, [session])

  return (
    <dialog
      ref={dialog}
      className="media-viewer"
      aria-label={label}
      onCancel={(event) => {
        event.preventDefault()
        dismiss.current()
      }}
    >
      <button
        type="button"
        className="media-viewer-close"
        autoFocus
        aria-label="Close media viewer"
      >
        ×
      </button>
      <div ref={stage} className="media-viewer-stage">
        {video ? (
          <video
            src={src}
            aria-label={label}
            autoPlay
            loop
            muted
            playsInline
            onLoadedMetadata={(event) => {
              event.currentTarget.currentTime = session.time
              void event.currentTarget.play().catch(() => undefined)
            }}
          />
        ) : (
          <img src={src} alt={label} />
        )}
      </div>
    </dialog>
  )
}
