// Solid page. Data reaches the component as `props.data` (same contract
// every adapter uses).
import HmrTarget from '../../components/hmr-target.tsx'

export default function Page(props: { data: { message: string } }) {
  return (
    <main>
      <h1>Home</h1>
      <p>{props.data.message}</p>
      <HmrTarget />
    </main>
  )
}
