interface Props {
  data?: { message?: string }
}
export default function Slow({ data }: Props) {
  return (
    <main data-slow="">
      <p data-slow-message="">{data?.message ?? 'no data'}</p>
    </main>
  )
}
