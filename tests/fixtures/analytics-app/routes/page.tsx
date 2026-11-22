// §V2-E06: the analytics fixture app's home route. The loader's
// route-injection scan picks this up; the `module_add` example does not
// depend on it (it exercises config-edit + manifest resolution), but a
// real `oxide build` against this fixture would serve `/`.
export default function Home() {
  return <main data-route="/">analytics fixture app</main>
}
