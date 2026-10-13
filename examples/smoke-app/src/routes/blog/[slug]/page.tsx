// The blog page reads its content from the loader's data (props.data).
// `oxide render` and `oxide dev` both run the loader first, so the title
// below comes from `page.server.ts` — not from URL params directly.
interface Props {
  params?: { slug?: string }
  data?: { title?: string; body?: string }
}

export default function Post(props: Props) {
  const title = props.data?.title ?? 'Untitled'
  const body = props.data?.body ?? ''
  return (
    <article>
      <h1>{title}</h1>
      <p>{body}</p>
    </article>
  )
}
