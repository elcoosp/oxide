// src/routes/blog/[slug]/loading.tsx — §8.4 loading boundary: streamed as
// the client-nav overlay while /__data is in flight.
export default function Loading() {
  return (
    <main className="container" style={{ paddingTop: 40 }}>
      <div className="loading-skeleton">loading post …</div>
    </main>
  )
}
