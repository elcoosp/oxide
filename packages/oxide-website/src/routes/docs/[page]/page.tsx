// src/routes/docs/[page]/page.tsx
import DocBody from '../doc-body.tsx'
import type { MdBlock } from '../../../lib/md.server.ts'

interface Props {
  data?: { doc?: { meta: { title: string; description: string }; html: MdBlock[] } }
}

export default function Page(props: Props) {
  return <DocBody doc={props.data?.doc} />
}
