export function fitMedia(
  width: number,
  height: number,
  viewportWidth: number,
  viewportHeight: number,
) {
  const margin = Math.min(32, viewportWidth / 20, viewportHeight / 20)
  const scale = Math.min(
    (viewportWidth - margin * 2) / width,
    (viewportHeight - margin * 2) / height,
  )
  return {
    width: width * scale,
    height: height * scale,
    left: (viewportWidth - width * scale) / 2,
    top: (viewportHeight - height * scale) / 2,
  }
}
