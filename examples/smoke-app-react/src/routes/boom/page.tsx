// §10.2 step 5 smoke fixture: this page throws during SSR so the nearest
// `error.tsx` (routes/error.tsx) renders with the thrown message.
export default function Boom() {
  throw new Error('Boom page exploded on purpose')
}
