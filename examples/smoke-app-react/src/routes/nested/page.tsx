interface Props {
  data?: { parentOrg?: string }
}
export default function NestedPage({ data }: Props) {
  return (
    <main data-nested-page="">
      <p data-parent-org="">{data?.parentOrg ?? 'NO DATA'}</p>
    </main>
  )
}
