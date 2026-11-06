# Using Images

Oxide ships a Rust-native image pipeline behind the `/_oxide/image`
endpoint and a cross-framework `<Image>` component per adapter.

## Two ways to reference images

### 1. Local imports with `?w=` (responsive variants)

~~~tsx
import hero from './hero.png?w=400,800,1200'

export default function Page() {
  return (
    <Image
      src={hero.src}
      srcset={hero.srcset}
      sizes="100vw"
      alt="Hero"
    />
  )
}
~~~

The `?w=` import is resolved at build time. The generated `srcset`
targets `/_oxide/image?url=/path/to/hero.png&w=<width>&format=webp`,
so the endpoint resizes on demand.

### 2. Static assets in `public/`

Files placed in `public/` are served verbatim at the site root:

~~~tsx
<img src="/logo.png" alt="Logo" />
~~~

They are copied to `dist/` during `oxide build` (not just at `oxide start`),
so a built site is deployable to a static host without any extra steps.

## The `/_oxide/image` endpoint

~~~
/_oxide/image?url=/logo.png&w=400&format=webp
~~~

| Param         | Values                        | Required |
|---------------|-------------------------------|----------|
| `url`         | Site-relative path (`/...`)   | yes      |
| `w`           | Target width in pixels        | optional |
| `h`           | Target height in pixels       | optional |
| `format`      | `webp`, `avif`, `jpeg`, `png` | optional |
| `quality`     | `0-100`                       | optional |
| `placeholder` | `lqip`, `blur`                | optional |

Note: `?src=` is **not** accepted — the canonical parameter is `?url=`.

## `<Image>` component (React / Vue / Solid)

~~~tsx
<Image
  src={logo.src}
  srcset={logo.srcset}
  width={120}
  height={120}
  sizes="(max-width: 768px) 50vw, 25vw"
  alt="Oxide"
  priority
/>
~~~

Full prop reference in `packages/react/src/image.ts`.
