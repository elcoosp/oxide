// The blog page renders the URL slug from props — proves the router's
// match, the pipeline's PageRenderInput, and the adapter all thread the
// parameter through. The props shape matches the adapter contract:
//   { route, url, params, data }
interface Props {
  params?: { slug?: string }
  data?: unknown
}

export default function Post(props: Props) {
  const slug = props.params?.slug ?? 'unknown'
  return <h1>Post: {slug}</h1>
}
