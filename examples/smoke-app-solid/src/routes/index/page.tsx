// Solid page. Data reaches the component as `props.data` (same contract
// every adapter uses).
export default function Page(props: { data: { message: string } }) {
  return (
    <main>
      <h1>Home</h1>
      <p>{props.data.message}</p>
    </main>
  )
}
