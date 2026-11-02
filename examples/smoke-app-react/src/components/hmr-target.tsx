import { useState } from 'react'

export default function HmrTarget() {
  const [count, setCount] = useState(0)
  return (
    <>
      <h1 data-hmr-target="">HMR-TARGET-ORIGINAL</h1>
      <button data-hmr-counter="" onClick={() => setCount(count + 1)}>{count}</button>
    </>
  )
}
