// src/routes/docs/[page]/page.tsx
import DocBody from '../doc-body.tsx'

interface Props {
  data?: { doc?: { meta: { title: string; description: string }; html: string } }
}

export default function Page(props: Props) {
  return <DocBody doc={props.data?.doc} />
}
