export default function Dash({ data }) {
  return (
    <main>
      <h1>Dashboard</h1>
      <p>Theme cookie: {data.theme ?? 'unset'}</p>
    </main>
  )
}
