// A plain `<form method="post" action="?/comment">`. Without client JS the
// browser submits it natively and the server re-renders this page with the
// action's result in `data.form`. With JS the adapter's `enhance` would
// intercept it — the smoke app does not wire `enhance`, which is deliberate:
// this fixture tests the no-JS path.
export default function Comment({ data }: { data?: Record<string, unknown> }) {
  const form = data?.form as { ok?: boolean; received?: string } | undefined
  return (
    <main data-comment="">
      <h1>Comment</h1>
      <form method="post" action="?/comment">
        <input name="text" defaultValue="" />
        <button type="submit">Send</button>
      </form>
      {form?.ok ? <p data-comment-result="">Received: {form.received}</p> : null}
    </main>
  )
}
