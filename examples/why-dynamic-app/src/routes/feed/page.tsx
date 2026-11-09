export default function Feed({ data }) {
  return (
    <main>
      <h1>Feed</h1>
      <ul>
        {(data.items ?? []).map((item) => (
          <li key={item.id}>{item.title}</li>
        ))}
      </ul>
    </main>
  )
}
