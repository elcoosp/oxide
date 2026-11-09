export default function SearchPage({ data }) {
  return (
    <main>
      <h1>Search</h1>
      <p>Query: {data.q ?? '(none)'}</p>
    </main>
  )
}
