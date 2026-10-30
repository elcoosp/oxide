/// <reference types="vite/client" />

// Image variant modules produced by `?w=` imports (Phase 2 I5)
declare module '*.png?w=*' {
  const m: { src: string; srcset: string; width: number; height: number }
  export default m
  export const src: string
  export const srcset: string
  export const width: number
  export const height: number
}

declare module '*.jpg?w=*' {
  const m: { src: string; srcset: string; width: number; height: number }
  export default m
  export const src: string
  export const srcset: string
  export const width: number
  export const height: number
}

declare module '*.webp?w=*' {
  const m: { src: string; srcset: string; width: number; height: number }
  export default m
}

declare module '*.avif?w=*' {
  const m: { src: string; srcset: string; width: number; height: number }
  export default m
}

// Plain asset imports (hashed URL strings)
declare module '*.png'  { const src: string; export default src }
declare module '*.jpg'  { const src: string; export default src }
declare module '*.jpeg' { const src: string; export default src }
declare module '*.webp' { const src: string; export default src }
declare module '*.avif' { const src: string; export default src }
declare module '*.gif'  { const src: string; export default src }
declare module '*.svg'  { const src: string; export default src }
