interface Props {
  data?: { greeting?: string; role?: string }
}
export default function Greeting({ data }: Props) {
  return (
    <main data-greeting="">
      <p data-greeting-text="">{data?.greeting ?? 'no data'}</p>
      <p data-greeting-role="">{data?.role ?? 'no role'}</p>
    </main>
  )
}
