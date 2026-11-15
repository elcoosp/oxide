// §V2-F15 fixture: renders the replay-sensitive data so the HTML carries
// every recorded value — a replay that misses any of them diverges.
export default function OxReplay({
  data,
}: {
  data?: { now: number; rand: number; uuid: string; fetchStatus: number; fetchBody: string }
}) {
  return (
    <div>
      <h1>ox-replay fixture</h1>
      <ul>
        <li id="ox-now">{String(data?.now ?? '')}</li>
        <li id="ox-rand">{String(data?.rand ?? '')}</li>
        <li id="ox-uuid">{data?.uuid ?? ''}</li>
        <li id="ox-fetch-status">{String(data?.fetchStatus ?? '')}</li>
        <li id="ox-fetch-body">{data?.fetchBody ?? ''}</li>
      </ul>
    </div>
  )
}
