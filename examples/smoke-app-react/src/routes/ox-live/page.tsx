// §V2-F05/F06 fixture: renders the live loader data so an SSE push is
// visible after the client applies the new snapshot in place (the same
// in-place update a navigation uses).
export default function OxLive({ data }: { data?: { n?: number } }) {
  return (
    <div>
      <h1>ox-live fixture</h1>
      <span id="ox-live-n">{String(data?.n ?? '')}</span>
    </div>
  )
}
