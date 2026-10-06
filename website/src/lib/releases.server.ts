import '@tanstack/react-start/server-only'
import { createReleaseService } from './releases'

export const getLatestRelease = createReleaseService()
