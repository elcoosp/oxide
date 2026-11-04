import { useState } from 'react'

// §1.2 acceptance fixture. Deliberately minimal: ONE hook (useState), then
// the test edits the file to add a SECOND hook (useEffect). Before §1.2's
// real $RefreshSig$, React Refresh could not detect this shape change, so
// it would do an in-place update and PRESERVE the counter (state stays 3
// after 3 clicks). With $RefreshSig$ recording the hook sequence, the
// runtime sees "useState → useState, useEffect" and REMOUNTS — counter
// resets to 0.
export default function HmrShapeTarget() {
  const [count, setCount] = useState(0)
  return (
    <>
      <h1 data-shape-target="">SHAPE-ORIGINAL</h1>
      <button data-shape-counter="" onClick={() => setCount((c) => c + 1)}>{count}</button>
    </>
  )
}
