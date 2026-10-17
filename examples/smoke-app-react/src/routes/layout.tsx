// §8.2 smoke-test layout: every page in the app nests inside this wrapper.
// The child page arrives on `props.children` as a live React element — the
// adapter expands the `{ Component, props }` descriptor to an element before
// calling `renderToString`, so this component is exactly what a real user
// layout looks like.
import type { ReactNode } from 'react'

interface Props {
  children?: ReactNode
}

export default function RootLayout({ children }: Props) {
  return (
    <div data-layout="root">
      <header data-layout-header="">
        <a href="/">Smoke</a>
      </header>
      <section data-layout-children="">{children}</section>
    </div>
  )
}
