import Link from 'next/link'
export function generateStaticParams() {
  return [{ slug: 'a' }]
}
export default function Post() {
  return <Link href="/">home</Link>
}
