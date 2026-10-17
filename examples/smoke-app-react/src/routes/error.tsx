// §10.2 step 5: nearest error.tsx wraps this throw. The adapter renders it
// instead of the generic OX-500 page.
interface Props {
  error?: { message?: string; route?: string; status?: number }
}
export default function ErrorPage({ error }: Props) {
  return (
    <main data-error-page="">
      <h1>Something went wrong</h1>
      <p data-error-message="">{error?.message ?? 'unknown error'}</p>
    </main>
  )
}
