interface Props {
  children?: unknown
}
export default function NestedLayout({ children }: Props) {
  return (
    <section data-nested-layout="">
      <span data-nested="yes">nested</span>
      {children as never}
    </section>
  )
}
