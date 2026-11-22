// @oxide/analytics — a fixture route the module ships so the loader's
// route-injection scan picks up `/modules/@oxide/analytics/health` (a
// health-check page that prints the configured endpoint). This is the
// spec's "each module has a fixture app" half — the route lives under
// the module's own routes dir.

export default function HealthPage() {
  return (
    <main data-route="/modules/@oxide/analytics/health">
      <h1>@oxide/analytics</h1>
      <p>The vitals beacon sink is wired. See the browser console for samples.</p>
    </main>
  )
}
