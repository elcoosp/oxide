// V2-C02 fixture (the smoke-route shape): typed props, a member-expression
// text hole and a component child — the `routes/index/page.tsx` shape the
// smoke app ships (the import is preserved verbatim; the compile is
// single-file, the bundler resolves it).
import HmrTarget from './HmrTarget.tsx';

export default function Page(props: { data: { message: string } }) {
  return (
    <main>
      <h1>Home</h1>
      <p>{props.data.message}</p>
      <HmrTarget />
    </main>
  );
}
