import { useState } from 'react'
import { utilValue } from '../lib/util.ts'

export default function HmrTarget() {
  const [count, setCount] = useState(0)
  return (
    <>
      <h1 data-hmr-target="">HMR-TARGET-ORIGINAL</h1>
      <span data-hmr-util="">{utilValue}</span>
      <button data-hmr-counter="" onClick={() => setCount(count + 1)}>{count}</button>
    </>
  )
}
